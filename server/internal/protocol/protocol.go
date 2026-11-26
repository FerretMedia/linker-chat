package protocol

import (
	"encoding/json"
	"time"
)

// Envelope is the top-level JSON frame sent over WebSockets.
type Envelope struct {
	Type  string          `json:"type"`
	ReqID string          `json:"req_id,omitempty"`
	Data  json.RawMessage `json:"data,omitempty"`
}

// User represents an authenticated chat identity (Clerk user, guest, or API token user).
type User struct {
	ID          string `json:"id"`
	Username    string `json:"username"`
	DisplayName string `json:"display_name"`
	AvatarURL   string `json:"avatar_url,omitempty"`
	Color       string `json:"color"`
	Role        string `json:"role,omitempty"`
	IsGuest     bool   `json:"is_guest"`
	IsAdmin     bool   `json:"is_admin,omitempty"`
}

// Emote represents a native custom emote stored on the server.
type Emote struct {
	Code     string `json:"code"`
	URL      string `json:"url"`
	Animated bool   `json:"animated"`
}

// RoomState describes mutable per-room settings and live count.
type RoomState struct {
	Room        string `json:"room"`
	Title       string `json:"title"`
	SlowModeSec int    `json:"slow_mode_sec"`
	AllowGuests bool   `json:"allow_guests"`
	ViewerCount int    `json:"viewer_count"`
}

// ChatMessage is the canonical native chat message payload.
type ChatMessage struct {
	ID        string    `json:"id"`
	Room      string    `json:"room"`
	User      User      `json:"user"`
	Content   string    `json:"content"`
	ReplyToID string    `json:"reply_to_id,omitempty"`
	Badges    []string  `json:"badges"`
	CreatedAt time.Time `json:"created_at"`
}

// --- Client -> Server payloads ---

type AuthRequest struct {
	Token string `json:"token"`
}

type AuthGuestRequest struct {
	Nickname   string `json:"nickname"`
	GuestToken string `json:"guest_token,omitempty"`
}

type AuthTokenRequest struct {
	Token       string `json:"token"`
	DisplayName string `json:"display_name,omitempty"`
}

type RoomRequest struct {
	Room string `json:"room"`
}

type SendMessageRequest struct {
	Room      string `json:"room"`
	Content   string `json:"content"`
	ReplyToID string `json:"reply_to_id,omitempty"`
}

type ModerateRequest struct {
	Room           string `json:"room"`
	Action         string `json:"action"` // delete | timeout | ban | unban | clear | set_role
	TargetUserID   string `json:"target_user_id,omitempty"`
	TargetUsername string `json:"target_username,omitempty"`
	MessageID      string `json:"message_id,omitempty"`
	DurationSec    int    `json:"duration_sec,omitempty"`
	Role           string `json:"role,omitempty"`
	Reason         string `json:"reason,omitempty"`
}

type PingRequest struct {
	TS int64 `json:"ts,omitempty"`
}

// --- Server -> Client payloads ---

type AuthOKPayload struct {
	User       User   `json:"user"`
	GuestToken string `json:"guest_token,omitempty"`
}

type RoomJoinedPayload struct {
	Room           string        `json:"room"`
	State          RoomState     `json:"state"`
	Role           string        `json:"role"`
	ViewerCount    int           `json:"viewer_count"`
	Emotes         []Emote       `json:"emotes"`
	RecentMessages []ChatMessage `json:"recent_messages"`
}

type MessageDeletedPayload struct {
	Room      string `json:"room"`
	MessageID string `json:"message_id"`
	ActorID   string `json:"actor_id"`
}

type UserModeratedPayload struct {
	Room           string `json:"room"`
	Action         string `json:"action"`
	TargetUserID   string `json:"target_user_id,omitempty"`
	TargetUsername string `json:"target_username,omitempty"`
	DurationSec    int    `json:"duration_sec,omitempty"`
	Reason         string `json:"reason,omitempty"`
	ActorID        string `json:"actor_id"`
}

type ChatClearedPayload struct {
	Room    string `json:"room"`
	ActorID string `json:"actor_id"`
}

type SystemPayload struct {
	Room string `json:"room,omitempty"`
	Text string `json:"text"`
}

type ErrorPayload struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

// MustMarshalEnvelope encodes a server event envelope.
func MustMarshalEnvelope(typ, reqID string, data any) []byte {
	var raw json.RawMessage
	if data != nil {
		b, err := json.Marshal(data)
		if err == nil {
			raw = b
		}
	}
	b, _ := json.Marshal(Envelope{
		Type:  typ,
		ReqID: reqID,
		Data:  raw,
	})
	return b
}
