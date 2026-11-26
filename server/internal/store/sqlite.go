package store

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/FerretMedia/linker-chat/server/internal/protocol"
	_ "modernc.org/sqlite"
)

// Store wraps SQLite persistence for Linker-chat.
type Store struct {
	db                 *sql.DB
	defaultAllowGuests bool
}

// RoomRecord holds persisted room configuration.
type RoomRecord struct {
	Slug        string
	Title       string
	SlowModeSec int
	AllowGuests bool
	CreatedAt   time.Time
}

// BanRecord holds an active ban or timeout.
type BanRecord struct {
	RoomSlug       string
	TargetUserID   string
	TargetUsername string
	Reason         string
	ActorID        string
	ExpiresAt      *time.Time
}

// APITokenRecord describes a stored personal API token.
type APITokenRecord struct {
	UserID      string
	Username    string
	DisplayName string
	Role        string
	IsAdmin     bool
}

// Open initializes SQLite in WAL mode and runs schema migrations.
func Open(path string, defaultAllowGuests bool) (*Store, error) {
	dsn := path
	if !strings.Contains(dsn, "?") && dsn != ":memory:" {
		dsn += "?_pragma=journal_mode(WAL)&_pragma=busy_timeout(5000)&_pragma=foreign_keys(ON)"
	}
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, fmt.Errorf("open sqlite: %w", err)
	}
	// SQLite works best with a single writer pool in Go when using WAL.
	db.SetMaxOpenConns(1)

	s := &Store{db: db, defaultAllowGuests: defaultAllowGuests}
	if err := s.migrate(context.Background()); err != nil {
		_ = db.Close()
		return nil, err
	}
	return s, nil
}

// Close closes the underlying database handle.
func (s *Store) Close() error {
	return s.db.Close()
}

func (s *Store) migrate(ctx context.Context) error {
	const schema = `
CREATE TABLE IF NOT EXISTS rooms (
	slug TEXT PRIMARY KEY,
	title TEXT NOT NULL,
	slow_mode_sec INTEGER NOT NULL DEFAULT 0,
	allow_guests INTEGER NOT NULL DEFAULT 1,
	created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS room_roles (
	room_slug TEXT NOT NULL,
	user_key TEXT NOT NULL,
	role TEXT NOT NULL,
	updated_at TEXT NOT NULL,
	PRIMARY KEY (room_slug, user_key)
);

CREATE TABLE IF NOT EXISTS bans (
	room_slug TEXT NOT NULL,
	target_key TEXT NOT NULL,
	target_user_id TEXT NOT NULL DEFAULT '',
	target_username TEXT NOT NULL DEFAULT '',
	reason TEXT NOT NULL DEFAULT '',
	actor_id TEXT NOT NULL DEFAULT '',
	expires_at TEXT,
	created_at TEXT NOT NULL,
	PRIMARY KEY (room_slug, target_key)
);

CREATE TABLE IF NOT EXISTS messages (
	id TEXT PRIMARY KEY,
	room_slug TEXT NOT NULL,
	user_id TEXT NOT NULL,
	username TEXT NOT NULL,
	display_name TEXT NOT NULL,
	avatar_url TEXT NOT NULL DEFAULT '',
	color TEXT NOT NULL,
	role TEXT NOT NULL DEFAULT 'member',
	is_guest INTEGER NOT NULL DEFAULT 0,
	badges_json TEXT NOT NULL DEFAULT '[]',
	content TEXT NOT NULL,
	reply_to_id TEXT NOT NULL DEFAULT '',
	created_at TEXT NOT NULL,
	deleted_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_messages_room_created ON messages (room_slug, created_at DESC);

CREATE TABLE IF NOT EXISTS room_emotes (
	room_slug TEXT NOT NULL,
	code TEXT NOT NULL,
	url TEXT NOT NULL,
	animated INTEGER NOT NULL DEFAULT 0,
	PRIMARY KEY (room_slug, code)
);

CREATE TABLE IF NOT EXISTS api_tokens (
	token_hash TEXT PRIMARY KEY,
	user_id TEXT NOT NULL,
	username TEXT NOT NULL,
	display_name TEXT NOT NULL,
	role TEXT NOT NULL DEFAULT 'member',
	is_admin INTEGER NOT NULL DEFAULT 0,
	label TEXT NOT NULL DEFAULT '',
	created_at TEXT NOT NULL
);
`
	_, err := s.db.ExecContext(ctx, schema)
	if err != nil {
		return fmt.Errorf("migrate sqlite: %w", err)
	}
	return nil
}

// EnsureRoom returns an existing room or creates it with defaults.
func (s *Store) EnsureRoom(ctx context.Context, slug string) (RoomRecord, error) {
	slug = normalizeSlug(slug)
	now := time.Now().UTC().Format(time.RFC3339Nano)
	allow := 0
	if s.defaultAllowGuests {
		allow = 1
	}
	_, err := s.db.ExecContext(ctx, `
		INSERT OR IGNORE INTO rooms (slug, title, slow_mode_sec, allow_guests, created_at)
		VALUES (?, ?, 0, ?, ?)
	`, slug, slug, allow, now)
	if err != nil {
		return RoomRecord{}, err
	}
	return s.GetRoom(ctx, slug)
}

// GetRoom fetches a room by slug.
func (s *Store) GetRoom(ctx context.Context, slug string) (RoomRecord, error) {
	slug = normalizeSlug(slug)
	var rec RoomRecord
	var allow int
	var createdStr string
	err := s.db.QueryRowContext(ctx, `
		SELECT slug, title, slow_mode_sec, allow_guests, created_at
		FROM rooms WHERE slug = ?
	`, slug).Scan(&rec.Slug, &rec.Title, &rec.SlowModeSec, &allow, &createdStr)
	if err != nil {
		return RoomRecord{}, err
	}
	rec.AllowGuests = allow != 0
	rec.CreatedAt, _ = time.Parse(time.RFC3339Nano, createdStr)
	return rec, nil
}

// UpdateRoomSettings updates slow mode and guest allowance for a room.
func (s *Store) UpdateRoomSettings(ctx context.Context, slug string, slowModeSec int, allowGuests bool) (RoomRecord, error) {
	slug = normalizeSlug(slug)
	if _, err := s.EnsureRoom(ctx, slug); err != nil {
		return RoomRecord{}, err
	}
	allow := 0
	if allowGuests {
		allow = 1
	}
	if slowModeSec < 0 {
		slowModeSec = 0
	}
	_, err := s.db.ExecContext(ctx, `
		UPDATE rooms SET slow_mode_sec = ?, allow_guests = ? WHERE slug = ?
	`, slowModeSec, allow, slug)
	if err != nil {
		return RoomRecord{}, err
	}
	return s.GetRoom(ctx, slug)
}

// GetUserRole looks up a user's room role by user ID or lowercase username.
func (s *Store) GetUserRole(ctx context.Context, roomSlug, userID, username string) string {
	roomSlug = normalizeSlug(roomSlug)
	keys := []string{strings.ToLower(strings.TrimSpace(userID)), strings.ToLower(strings.TrimSpace(username))}
	for _, k := range keys {
		if k == "" {
			continue
		}
		var role string
		err := s.db.QueryRowContext(ctx, `
			SELECT role FROM room_roles WHERE room_slug = ? AND user_key = ?
		`, roomSlug, k).Scan(&role)
		if err == nil && role != "" {
			return role
		}
	}
	return ""
}

// SetUserRole assigns or removes a role for a user key (ID or username).
func (s *Store) SetUserRole(ctx context.Context, roomSlug, userKey, role string) error {
	roomSlug = normalizeSlug(roomSlug)
	userKey = strings.ToLower(strings.TrimSpace(userKey))
	if userKey == "" {
		return errors.New("empty user key")
	}
	if role == "" || role == "member" {
		_, err := s.db.ExecContext(ctx, `DELETE FROM room_roles WHERE room_slug = ? AND user_key = ?`, roomSlug, userKey)
		return err
	}
	now := time.Now().UTC().Format(time.RFC3339Nano)
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO room_roles (room_slug, user_key, role, updated_at)
		VALUES (?, ?, ?, ?)
		ON CONFLICT(room_slug, user_key) DO UPDATE SET role = excluded.role, updated_at = excluded.updated_at
	`, roomSlug, userKey, role, now)
	return err
}

// UpsertBan stores a timeout (durationSec > 0) or permanent ban (durationSec <= 0).
func (s *Store) UpsertBan(ctx context.Context, roomSlug, targetUserID, targetUsername, reason, actorID string, durationSec int) error {
	roomSlug = normalizeSlug(roomSlug)
	key := strings.ToLower(strings.TrimSpace(targetUserID))
	if key == "" {
		key = strings.ToLower(strings.TrimSpace(targetUsername))
	}
	if key == "" {
		return errors.New("missing ban target")
	}
	var expires sql.NullString
	if durationSec > 0 {
		expires = sql.NullString{
			String: time.Now().UTC().Add(time.Duration(durationSec) * time.Second).Format(time.RFC3339Nano),
			Valid:  true,
		}
	}
	now := time.Now().UTC().Format(time.RFC3339Nano)
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO bans (room_slug, target_key, target_user_id, target_username, reason, actor_id, expires_at, created_at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT(room_slug, target_key) DO UPDATE SET
			target_user_id = excluded.target_user_id,
			target_username = excluded.target_username,
			reason = excluded.reason,
			actor_id = excluded.actor_id,
			expires_at = excluded.expires_at,
			created_at = excluded.created_at
	`, roomSlug, key, targetUserID, strings.ToLower(targetUsername), reason, actorID, expires, now)
	return err
}

// RemoveBan lifts any ban or timeout matching user ID or username.
func (s *Store) RemoveBan(ctx context.Context, roomSlug, targetUserID, targetUsername string) error {
	roomSlug = normalizeSlug(roomSlug)
	idKey := strings.ToLower(strings.TrimSpace(targetUserID))
	nameKey := strings.ToLower(strings.TrimSpace(targetUsername))
	_, err := s.db.ExecContext(ctx, `
		DELETE FROM bans
		WHERE room_slug = ? AND (
			(target_key != '' AND (target_key = ? OR target_key = ?))
			OR (target_username != '' AND target_username = ?)
		)
	`, roomSlug, idKey, nameKey, nameKey)
	return err
}

// GetActiveBan returns an active ban/timeout for the given user, clearing expired timeouts.
func (s *Store) GetActiveBan(ctx context.Context, roomSlug, userID, username string) (*BanRecord, error) {
	roomSlug = normalizeSlug(roomSlug)
	idKey := strings.ToLower(strings.TrimSpace(userID))
	nameKey := strings.ToLower(strings.TrimSpace(username))

	var rec BanRecord
	var targetKey string
	var expires sql.NullString
	err := s.db.QueryRowContext(ctx, `
		SELECT target_key, target_user_id, target_username, reason, actor_id, expires_at
		FROM bans
		WHERE room_slug = ? AND (target_key = ? OR target_key = ? OR (target_username != '' AND target_username = ?))
		LIMIT 1
	`, roomSlug, idKey, nameKey, nameKey).Scan(&targetKey, &rec.TargetUserID, &rec.TargetUsername, &rec.Reason, &rec.ActorID, &expires)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	rec.RoomSlug = roomSlug
	if expires.Valid && expires.String != "" {
		t, parseErr := time.Parse(time.RFC3339Nano, expires.String)
		if parseErr == nil {
			if time.Now().UTC().After(t) {
				_, _ = s.db.ExecContext(ctx, `DELETE FROM bans WHERE room_slug = ? AND target_key = ?`, roomSlug, targetKey)
				return nil, nil
			}
			rec.ExpiresAt = &t
		}
	}
	return &rec, nil
}

// SaveMessage persists a chat message.
func (s *Store) SaveMessage(ctx context.Context, msg protocol.ChatMessage) error {
	badgesJSON, _ := json.Marshal(msg.Badges)
	isGuest := 0
	if msg.User.IsGuest {
		isGuest = 1
	}
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO messages (
			id, room_slug, user_id, username, display_name, avatar_url, color, role, is_guest,
			badges_json, content, reply_to_id, created_at
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
	`,
		msg.ID,
		normalizeSlug(msg.Room),
		msg.User.ID,
		msg.User.Username,
		msg.User.DisplayName,
		msg.User.AvatarURL,
		msg.User.Color,
		msg.User.Role,
		isGuest,
		string(badgesJSON),
		msg.Content,
		msg.ReplyToID,
		msg.CreatedAt.UTC().Format(time.RFC3339Nano),
	)
	return err
}

// GetRecentMessages returns up to limit non-deleted messages in chronological order.
func (s *Store) GetRecentMessages(ctx context.Context, roomSlug string, limit int) ([]protocol.ChatMessage, error) {
	if limit <= 0 {
		limit = 50
	}
	rows, err := s.db.QueryContext(ctx, `
		SELECT id, room_slug, user_id, username, display_name, avatar_url, color, role, is_guest,
		       badges_json, content, reply_to_id, created_at
		FROM messages
		WHERE room_slug = ? AND deleted_at IS NULL
		ORDER BY created_at DESC
		LIMIT ?
	`, normalizeSlug(roomSlug), limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var rev []protocol.ChatMessage
	for rows.Next() {
		var m protocol.ChatMessage
		var isGuest int
		var badgesRaw, createdRaw string
		if err := rows.Scan(
			&m.ID, &m.Room, &m.User.ID, &m.User.Username, &m.User.DisplayName,
			&m.User.AvatarURL, &m.User.Color, &m.User.Role, &isGuest,
			&badgesRaw, &m.Content, &m.ReplyToID, &createdRaw,
		); err != nil {
			return nil, err
		}
		m.User.IsGuest = isGuest != 0
		_ = json.Unmarshal([]byte(badgesRaw), &m.Badges)
		if m.Badges == nil {
			m.Badges = []string{}
		}
		m.CreatedAt, _ = time.Parse(time.RFC3339Nano, createdRaw)
		rev = append(rev, m)
	}
	// Reverse to chronological order (oldest -> newest).
	for i, j := 0, len(rev)-1; i < j; i, j = i+1, j-1 {
		rev[i], rev[j] = rev[j], rev[i]
	}
	if rev == nil {
		rev = []protocol.ChatMessage{}
	}
	return rev, rows.Err()
}

// MarkMessageDeleted soft-deletes a message in a room.
func (s *Store) MarkMessageDeleted(ctx context.Context, roomSlug, messageID string) error {
	now := time.Now().UTC().Format(time.RFC3339Nano)
	_, err := s.db.ExecContext(ctx, `
		UPDATE messages SET deleted_at = ? WHERE room_slug = ? AND id = ?
	`, now, normalizeSlug(roomSlug), messageID)
	return err
}

// ClearRoomMessages soft-deletes all active messages in a room.
func (s *Store) ClearRoomMessages(ctx context.Context, roomSlug string) error {
	now := time.Now().UTC().Format(time.RFC3339Nano)
	_, err := s.db.ExecContext(ctx, `
		UPDATE messages SET deleted_at = ? WHERE room_slug = ? AND deleted_at IS NULL
	`, now, normalizeSlug(roomSlug))
	return err
}

// ListEmotes returns room-specific and global ("_global") native emotes.
func (s *Store) ListEmotes(ctx context.Context, roomSlug string) ([]protocol.Emote, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT code, url, animated FROM room_emotes
		WHERE room_slug = ? OR room_slug = '_global'
		ORDER BY code ASC
	`, normalizeSlug(roomSlug))
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	emotes := []protocol.Emote{}
	for rows.Next() {
		var e protocol.Emote
		var anim int
		if err := rows.Scan(&e.Code, &e.URL, &anim); err != nil {
			return nil, err
		}
		e.Animated = anim != 0
		emotes = append(emotes, e)
	}
	return emotes, rows.Err()
}

// UpsertEmote creates or updates a custom native emote for a room (or "_global").
func (s *Store) UpsertEmote(ctx context.Context, roomSlug string, e protocol.Emote) error {
	anim := 0
	if e.Animated {
		anim = 1
	}
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO room_emotes (room_slug, code, url, animated)
		VALUES (?, ?, ?, ?)
		ON CONFLICT(room_slug, code) DO UPDATE SET url = excluded.url, animated = excluded.animated
	`, normalizeSlug(roomSlug), strings.TrimSpace(e.Code), strings.TrimSpace(e.URL), anim)
	return err
}

// CreateAPIToken mints a new random personal API token and persists its SHA-256 hash.
func (s *Store) CreateAPIToken(ctx context.Context, rec APITokenRecord, label string) (string, error) {
	raw := make([]byte, 24)
	if _, err := rand.Read(raw); err != nil {
		return "", err
	}
	plain := "lct_" + hex.EncodeToString(raw)
	hash := hashToken(plain)
	isAdmin := 0
	if rec.IsAdmin {
		isAdmin = 1
	}
	now := time.Now().UTC().Format(time.RFC3339Nano)
	_, err := s.db.ExecContext(ctx, `
		INSERT INTO api_tokens (token_hash, user_id, username, display_name, role, is_admin, label, created_at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?)
	`, hash, rec.UserID, rec.Username, rec.DisplayName, rec.Role, isAdmin, label, now)
	if err != nil {
		return "", err
	}
	return plain, nil
}

// ValidateAPIToken resolves a plaintext personal API token.
func (s *Store) ValidateAPIToken(ctx context.Context, plain string) (*APITokenRecord, error) {
	hash := hashToken(plain)
	var rec APITokenRecord
	var isAdmin int
	err := s.db.QueryRowContext(ctx, `
		SELECT user_id, username, display_name, role, is_admin
		FROM api_tokens WHERE token_hash = ?
	`, hash).Scan(&rec.UserID, &rec.Username, &rec.DisplayName, &rec.Role, &isAdmin)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	rec.IsAdmin = isAdmin != 0
	return &rec, nil
}

func hashToken(plain string) string {
	sum := sha256.Sum256([]byte(plain))
	return hex.EncodeToString(sum[:])
}

func normalizeSlug(s string) string {
	s = strings.ToLower(strings.TrimSpace(s))
	if s == "" {
		return "main"
	}
	return s
}
