package hub

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/FerretMedia/linker-chat/server/internal/config"
	"github.com/FerretMedia/linker-chat/server/internal/protocol"
	"github.com/FerretMedia/linker-chat/server/internal/store"
)

// Client represents a connected WebSocket session inside the Hub.
type Client struct {
	ID   string
	Send chan []byte

	mu        sync.RWMutex
	user      *protocol.User
	rooms     map[string]struct{}
	lastSent  map[string]time.Time
	msgWindow []time.Time
}

// NewClient allocates a client with a bounded outbound queue.
func NewClient() *Client {
	buf := make([]byte, 8)
	_, _ = rand.Read(buf)
	return &Client{
		ID:       hex.EncodeToString(buf),
		Send:     make(chan []byte, 128),
		rooms:    make(map[string]struct{}),
		lastSent: make(map[string]time.Time),
	}
}

func (c *Client) SetUser(u protocol.User) {
	c.mu.Lock()
	defer c.mu.Unlock()
	cp := u
	c.user = &cp
}

func (c *Client) User() (protocol.User, bool) {
	c.mu.RLock()
	defer c.mu.RUnlock()
	if c.user == nil {
		return protocol.User{}, false
	}
	return *c.user, true
}

func (c *Client) Rooms() []string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	out := make([]string, 0, len(c.rooms))
	for r := range c.rooms {
		out = append(out, r)
	}
	return out
}

func (c *Client) TrySend(payload []byte) {
	select {
	case c.Send <- payload:
	default:
		// Drop for slow consumers instead of blocking room broadcast.
	}
}

func (c *Client) allowBurst(now time.Time, maxPer10s int) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	cutoff := now.Add(-10 * time.Second)
	kept := c.msgWindow[:0]
	for _, ts := range c.msgWindow {
		if ts.After(cutoff) {
			kept = append(kept, ts)
		}
	}
	if len(kept) >= maxPer10s {
		c.msgWindow = kept
		return false
	}
	c.msgWindow = append(kept, now)
	return true
}

func (c *Client) checkSlowMode(room string, slowModeSec int, now time.Time) time.Duration {
	if slowModeSec <= 0 {
		return 0
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if prev, ok := c.lastSent[room]; ok {
		wait := time.Duration(slowModeSec)*time.Second - now.Sub(prev)
		if wait > 0 {
			return wait
		}
	}
	c.lastSent[room] = now
	return 0
}

// Hub coordinates room memberships, message fanout, and moderation.
type Hub struct {
	cfg   config.Config
	store *store.Store

	mu    sync.RWMutex
	rooms map[string]map[*Client]struct{}
}

// New creates a new Hub.
func New(cfg config.Config, st *store.Store) *Hub {
	return &Hub{
		cfg:   cfg,
		store: st,
		rooms: make(map[string]map[*Client]struct{}),
	}
}

// Join subscribes a client to a room and returns the room snapshot.
func (h *Hub) Join(ctx context.Context, c *Client, roomSlug string) (protocol.RoomJoinedPayload, error) {
	roomRec, err := h.store.EnsureRoom(ctx, roomSlug)
	if err != nil {
		return protocol.RoomJoinedPayload{}, err
	}
	slug := roomRec.Slug

	h.mu.Lock()
	if h.rooms[slug] == nil {
		h.rooms[slug] = make(map[*Client]struct{})
	}
	h.rooms[slug][c] = struct{}{}
	viewerCount := len(h.rooms[slug])
	h.mu.Unlock()

	c.mu.Lock()
	c.rooms[slug] = struct{}{}
	c.mu.Unlock()

	recent, err := h.store.GetRecentMessages(ctx, slug, h.cfg.HistoryLimit)
	if err != nil {
		return protocol.RoomJoinedPayload{}, err
	}
	emotes, err := h.store.ListEmotes(ctx, slug)
	if err != nil {
		return protocol.RoomJoinedPayload{}, err
	}

	role := "guest"
	if u, ok := c.User(); ok {
		role = h.effectiveRole(ctx, slug, u)
	}

	state := protocol.RoomState{
		Room:        slug,
		Title:       roomRec.Title,
		SlowModeSec: roomRec.SlowModeSec,
		AllowGuests: roomRec.AllowGuests,
		ViewerCount: viewerCount,
	}

	// Notify existing members of updated viewer count.
	h.Broadcast(slug, protocol.MustMarshalEnvelope("room_state", "", state))

	return protocol.RoomJoinedPayload{
		Room:           slug,
		State:          state,
		Role:           role,
		ViewerCount:    viewerCount,
		Emotes:         emotes,
		RecentMessages: recent,
	}, nil
}

// Leave unsubscribes a client from a room.
func (h *Hub) Leave(ctx context.Context, c *Client, roomSlug string) {
	slug := normalizeSlug(roomSlug)

	h.mu.Lock()
	if members, ok := h.rooms[slug]; ok {
		delete(members, c)
		if len(members) == 0 {
			delete(h.rooms, slug)
		}
	}
	count := len(h.rooms[slug])
	h.mu.Unlock()

	c.mu.Lock()
	delete(c.rooms, slug)
	c.mu.Unlock()

	if count > 0 {
		if roomRec, err := h.store.GetRoom(ctx, slug); err == nil {
			h.Broadcast(slug, protocol.MustMarshalEnvelope("room_state", "", protocol.RoomState{
				Room:        slug,
				Title:       roomRec.Title,
				SlowModeSec: roomRec.SlowModeSec,
				AllowGuests: roomRec.AllowGuests,
				ViewerCount: count,
			}))
		}
	}
}

// Disconnect removes a client from all rooms.
func (h *Hub) Disconnect(ctx context.Context, c *Client) {
	for _, r := range c.Rooms() {
		h.Leave(ctx, c, r)
	}
}

// ViewerCount returns the active WebSocket subscriber count for a room.
func (h *Hub) ViewerCount(roomSlug string) int {
	h.mu.RLock()
	defer h.mu.RUnlock()
	return len(h.rooms[normalizeSlug(roomSlug)])
}

// Broadcast fans out a pre-encoded WebSocket frame to every client in a room.
func (h *Hub) Broadcast(roomSlug string, frame []byte) {
	slug := normalizeSlug(roomSlug)
	h.mu.RLock()
	members := h.rooms[slug]
	snapshot := make([]*Client, 0, len(members))
	for cl := range members {
		snapshot = append(snapshot, cl)
	}
	h.mu.RUnlock()

	for _, cl := range snapshot {
		cl.TrySend(frame)
	}
}

// HandleSendMessage validates permissions, runs slash commands, persists, and broadcasts a chat message.
func (h *Hub) HandleSendMessage(ctx context.Context, c *Client, reqID string, req protocol.SendMessageRequest) {
	u, ok := c.User()
	if !ok {
		c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
			Code:    "unauthorized",
			Message: "Sign in with Linker Clerk or choose a guest nickname before chatting.",
		}))
		return
	}

	roomRec, err := h.store.EnsureRoom(ctx, req.Room)
	if err != nil {
		c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
			Code:    "internal_error",
			Message: "Unable to load room.",
		}))
		return
	}
	slug := roomRec.Slug

	if u.IsGuest && !roomRec.AllowGuests {
		c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
			Code:    "guests_disabled",
			Message: "Guest chatting is disabled in this room. Please sign in with Linker Clerk.",
		}))
		return
	}

	role := h.effectiveRole(ctx, slug, u)

	// Check active bans / timeouts (mods and broadcasters bypass).
	if !isPrivileged(role) {
		ban, err := h.store.GetActiveBan(ctx, slug, u.ID, u.Username)
		if err == nil && ban != nil {
			code := "banned"
			msg := "You are banned from chatting in this room."
			if ban.ExpiresAt != nil {
				code = "timed_out"
				remaining := int(time.Until(*ban.ExpiresAt).Seconds())
				if remaining < 1 {
					remaining = 1
				}
				msg = fmt.Sprintf("You are timed out for %ds.", remaining)
			}
			c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
				Code:    code,
				Message: msg,
			}))
			return
		}
	}

	content := strings.TrimSpace(req.Content)
	if content == "" {
		return
	}
	if len([]rune(content)) > h.cfg.MaxMessageLength {
		c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
			Code:    "bad_request",
			Message: fmt.Sprintf("Message exceeds %d character limit.", h.cfg.MaxMessageLength),
		}))
		return
	}

	// Handle slash moderation commands before persisting as chat text.
	if strings.HasPrefix(content, "/") {
		if h.trySlashCommand(ctx, c, reqID, slug, u, role, content) {
			return
		}
	}

	now := time.Now().UTC()
	if !isPrivileged(role) {
		if !c.allowBurst(now, 8) {
			c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
				Code:    "rate_limited",
				Message: "You are sending messages too quickly.",
			}))
			return
		}
		if wait := c.checkSlowMode(slug, roomRec.SlowModeSec, now); wait > 0 {
			c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
				Code:    "slow_mode",
				Message: fmt.Sprintf("Slow mode is active. Wait %ds.", int(wait.Seconds())+1),
			}))
			return
		}
	}

	u.Role = role
	badges := badgesForRole(role, u.IsGuest)
	msg := protocol.ChatMessage{
		ID:        newMessageID(),
		Room:      slug,
		User:      u,
		Content:   content,
		ReplyToID: strings.TrimSpace(req.ReplyToID),
		Badges:    badges,
		CreatedAt: now,
	}

	if err := h.store.SaveMessage(ctx, msg); err != nil {
		c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
			Code:    "internal_error",
			Message: "Failed to persist message.",
		}))
		return
	}

	h.Broadcast(slug, protocol.MustMarshalEnvelope("message", "", msg))
}

// HandleModerate performs an explicit moderation action from Web or GPUI desktop clients.
func (h *Hub) HandleModerate(ctx context.Context, c *Client, reqID string, req protocol.ModerateRequest) {
	u, ok := c.User()
	if !ok {
		c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
			Code: "unauthorized", Message: "Authentication required.",
		}))
		return
	}
	slug := normalizeSlug(req.Room)
	role := h.effectiveRole(ctx, slug, u)
	if !isPrivileged(role) {
		c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
			Code: "forbidden", Message: "Moderator or broadcaster role required.",
		}))
		return
	}

	switch req.Action {
	case "delete":
		if req.MessageID == "" {
			return
		}
		_ = h.store.MarkMessageDeleted(ctx, slug, req.MessageID)
		h.Broadcast(slug, protocol.MustMarshalEnvelope("message_deleted", "", protocol.MessageDeletedPayload{
			Room: slug, MessageID: req.MessageID, ActorID: u.ID,
		}))
	case "clear":
		_ = h.store.ClearRoomMessages(ctx, slug)
		h.Broadcast(slug, protocol.MustMarshalEnvelope("chat_cleared", "", protocol.ChatClearedPayload{
			Room: slug, ActorID: u.ID,
		}))
	case "timeout":
		dur := req.DurationSec
		if dur <= 0 {
			dur = 600
		}
		if err := h.store.UpsertBan(ctx, slug, req.TargetUserID, req.TargetUsername, req.Reason, u.ID, dur); err != nil {
			c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{Code: "bad_request", Message: err.Error()}))
			return
		}
		h.Broadcast(slug, protocol.MustMarshalEnvelope("user_moderated", "", protocol.UserModeratedPayload{
			Room: slug, Action: "timeout", TargetUserID: req.TargetUserID, TargetUsername: req.TargetUsername,
			DurationSec: dur, Reason: req.Reason, ActorID: u.ID,
		}))
	case "ban":
		if err := h.store.UpsertBan(ctx, slug, req.TargetUserID, req.TargetUsername, req.Reason, u.ID, 0); err != nil {
			c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{Code: "bad_request", Message: err.Error()}))
			return
		}
		h.Broadcast(slug, protocol.MustMarshalEnvelope("user_moderated", "", protocol.UserModeratedPayload{
			Room: slug, Action: "ban", TargetUserID: req.TargetUserID, TargetUsername: req.TargetUsername,
			Reason: req.Reason, ActorID: u.ID,
		}))
	case "unban":
		_ = h.store.RemoveBan(ctx, slug, req.TargetUserID, req.TargetUsername)
		h.Broadcast(slug, protocol.MustMarshalEnvelope("user_moderated", "", protocol.UserModeratedPayload{
			Room: slug, Action: "unban", TargetUserID: req.TargetUserID, TargetUsername: req.TargetUsername,
			ActorID: u.ID,
		}))
	case "set_role":
		if role != "broadcaster" {
			c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
				Code: "forbidden", Message: "Only broadcasters can assign roles.",
			}))
			return
		}
		target := req.TargetUserID
		if target == "" {
			target = req.TargetUsername
		}
		if err := h.store.SetUserRole(ctx, slug, target, req.Role); err != nil {
			c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{Code: "bad_request", Message: err.Error()}))
			return
		}
		c.TrySend(protocol.MustMarshalEnvelope("system", reqID, protocol.SystemPayload{
			Room: slug, Text: fmt.Sprintf("Set role for %s to %s.", target, req.Role),
		}))
	default:
		c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{
			Code: "bad_request", Message: "Unknown moderation action.",
		}))
	}
}

func (h *Hub) trySlashCommand(ctx context.Context, c *Client, reqID, slug string, u protocol.User, role, content string) bool {
	fields := strings.Fields(content)
	if len(fields) == 0 {
		return false
	}
	cmd := strings.ToLower(fields[0])
	switch cmd {
	case "/clear":
		h.HandleModerate(ctx, c, reqID, protocol.ModerateRequest{Room: slug, Action: "clear"})
		return true
	case "/timeout":
		if len(fields) < 2 {
			c.TrySend(protocol.MustMarshalEnvelope("system", reqID, protocol.SystemPayload{Room: slug, Text: "Usage: /timeout <username> [seconds] [reason]"}))
			return true
		}
		dur := 600
		reasonStart := 2
		if len(fields) >= 3 {
			if parsed, err := strconv.Atoi(fields[2]); err == nil && parsed > 0 {
				dur = parsed
				reasonStart = 3
			}
		}
		reason := ""
		if len(fields) > reasonStart {
			reason = strings.Join(fields[reasonStart:], " ")
		}
		h.HandleModerate(ctx, c, reqID, protocol.ModerateRequest{
			Room: slug, Action: "timeout", TargetUsername: fields[1], DurationSec: dur, Reason: reason,
		})
		return true
	case "/ban":
		if len(fields) < 2 {
			c.TrySend(protocol.MustMarshalEnvelope("system", reqID, protocol.SystemPayload{Room: slug, Text: "Usage: /ban <username> [reason]"}))
			return true
		}
		reason := strings.Join(fields[2:], " ")
		h.HandleModerate(ctx, c, reqID, protocol.ModerateRequest{
			Room: slug, Action: "ban", TargetUsername: fields[1], Reason: reason,
		})
		return true
	case "/unban":
		if len(fields) < 2 {
			return true
		}
		h.HandleModerate(ctx, c, reqID, protocol.ModerateRequest{
			Room: slug, Action: "unban", TargetUsername: fields[1],
		})
		return true
	case "/mod", "/unmod", "/vip":
		if len(fields) < 2 {
			return true
		}
		newRole := "moderator"
		if cmd == "/unmod" {
			newRole = "member"
		} else if cmd == "/vip" {
			newRole = "vip"
		}
		h.HandleModerate(ctx, c, reqID, protocol.ModerateRequest{
			Room: slug, Action: "set_role", TargetUsername: fields[1], Role: newRole,
		})
		return true
	case "/slow":
		if !isPrivileged(role) {
			c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{Code: "forbidden", Message: "Moderator role required."}))
			return true
		}
		sec := 0
		if len(fields) >= 2 {
			sec, _ = strconv.Atoi(fields[1])
		}
		roomRec, err := h.store.EnsureRoom(ctx, slug)
		if err == nil {
			updated, err := h.store.UpdateRoomSettings(ctx, slug, sec, roomRec.AllowGuests)
			if err == nil {
				h.Broadcast(slug, protocol.MustMarshalEnvelope("room_state", "", protocol.RoomState{
					Room: slug, Title: updated.Title, SlowModeSec: updated.SlowModeSec,
					AllowGuests: updated.AllowGuests, ViewerCount: h.ViewerCount(slug),
				}))
			}
		}
		return true
	case "/guests":
		if !isPrivileged(role) {
			c.TrySend(protocol.MustMarshalEnvelope("error", reqID, protocol.ErrorPayload{Code: "forbidden", Message: "Moderator role required."}))
			return true
		}
		if len(fields) < 2 {
			return true
		}
		allow := strings.EqualFold(fields[1], "on") || strings.EqualFold(fields[1], "true")
		roomRec, err := h.store.EnsureRoom(ctx, slug)
		if err == nil {
			updated, err := h.store.UpdateRoomSettings(ctx, slug, roomRec.SlowModeSec, allow)
			if err == nil {
				h.Broadcast(slug, protocol.MustMarshalEnvelope("room_state", "", protocol.RoomState{
					Room: slug, Title: updated.Title, SlowModeSec: updated.SlowModeSec,
					AllowGuests: updated.AllowGuests, ViewerCount: h.ViewerCount(slug),
				}))
			}
		}
		return true
	default:
		return false
	}
}

func (h *Hub) effectiveRole(ctx context.Context, roomSlug string, u protocol.User) string {
	if u.IsAdmin || u.Role == "broadcaster" {
		return "broadcaster"
	}
	if dbRole := h.store.GetUserRole(ctx, roomSlug, u.ID, u.Username); dbRole != "" {
		return dbRole
	}
	if u.Role != "" {
		return u.Role
	}
	if u.IsGuest {
		return "guest"
	}
	return "member"
}

func isPrivileged(role string) bool {
	return role == "broadcaster" || role == "moderator"
}

func badgesForRole(role string, isGuest bool) []string {
	switch role {
	case "broadcaster":
		return []string{"broadcaster"}
	case "moderator":
		return []string{"moderator"}
	case "vip":
		return []string{"vip"}
	default:
		if isGuest {
			return []string{"guest"}
		}
		return []string{}
	}
}

func newMessageID() string {
	buf := make([]byte, 8)
	_, _ = rand.Read(buf)
	return fmt.Sprintf("msg_%d_%s", time.Now().UTC().UnixMilli(), hex.EncodeToString(buf[:4]))
}

func normalizeSlug(s string) string {
	s = strings.ToLower(strings.TrimSpace(s))
	if s == "" {
		return "main"
	}
	return s
}
