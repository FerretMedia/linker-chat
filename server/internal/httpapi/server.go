package httpapi

import (
	"context"
	"encoding/json"
	"log/slog"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/coder/websocket"
	"github.com/FerretMedia/linker-chat/server/internal/auth"
	"github.com/FerretMedia/linker-chat/server/internal/config"
	"github.com/FerretMedia/linker-chat/server/internal/hub"
	"github.com/FerretMedia/linker-chat/server/internal/protocol"
	"github.com/FerretMedia/linker-chat/server/internal/store"
)

// Server wires HTTP REST endpoints and WebSocket chat connections.
type Server struct {
	cfg      config.Config
	store    *store.Store
	auth     *auth.Verifier
	hub      *hub.Hub
	logger   *slog.Logger
	mux      *http.ServeMux
}

// New builds the HTTP/WS handler.
func New(cfg config.Config, st *store.Store, ver *auth.Verifier, h *hub.Hub, logger *slog.Logger) *Server {
	if logger == nil {
		logger = slog.Default()
	}
	s := &Server{
		cfg:    cfg,
		store:  st,
		auth:   ver,
		hub:    h,
		logger: logger,
		mux:    http.NewServeMux(),
	}
	s.routes()
	return s
}

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	s.setCORS(w, r)
	if r.Method == http.MethodOptions {
		w.WriteHeader(http.StatusNoContent)
		return
	}
	s.mux.ServeHTTP(w, r)
}

func (s *Server) routes() {
	s.mux.HandleFunc("GET /healthz", s.handleHealth)
	s.mux.HandleFunc("GET /ws", s.handleWS)
	s.mux.HandleFunc("GET /api/v1/rooms/{slug}", s.handleGetRoom)
	s.mux.HandleFunc("GET /api/v1/rooms/{slug}/messages", s.handleGetMessages)
	s.mux.HandleFunc("POST /api/v1/rooms/{slug}/emotes", s.handleUpsertEmote)
	s.mux.HandleFunc("POST /api/v1/tokens", s.handleMintToken)
}

func (s *Server) handleHealth(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, http.StatusOK, map[string]any{
		"status":  "ok",
		"service": "linker-chat-server",
	})
}

func (s *Server) handleGetRoom(w http.ResponseWriter, r *http.Request) {
	slug := r.PathValue("slug")
	roomRec, err := s.store.EnsureRoom(r.Context(), slug)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	emotes, err := s.store.ListEmotes(r.Context(), roomRec.Slug)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"room": protocol.RoomState{
			Room:        roomRec.Slug,
			Title:       roomRec.Title,
			SlowModeSec: roomRec.SlowModeSec,
			AllowGuests: roomRec.AllowGuests,
			ViewerCount: s.hub.ViewerCount(roomRec.Slug),
		},
		"emotes": emotes,
	})
}

func (s *Server) handleGetMessages(w http.ResponseWriter, r *http.Request) {
	slug := r.PathValue("slug")
	limit := s.cfg.HistoryLimit
	if q := r.URL.Query().Get("limit"); q != "" {
		if parsed, err := strconv.Atoi(q); err == nil && parsed > 0 && parsed <= 200 {
			limit = parsed
		}
	}
	msgs, err := s.store.GetRecentMessages(r.Context(), slug, limit)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"messages": msgs})
}

func (s *Server) handleUpsertEmote(w http.ResponseWriter, r *http.Request) {
	u, err := s.authenticateBearer(r)
	if err != nil {
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": err.Error()})
		return
	}
	slug := r.PathValue("slug")
	role := s.store.GetUserRole(r.Context(), slug, u.ID, u.Username)
	if !u.IsAdmin && role != "broadcaster" && role != "moderator" {
		writeJSON(w, http.StatusForbidden, map[string]string{"error": "moderator or broadcaster role required"})
		return
	}
	var e protocol.Emote
	if err := json.NewDecoder(r.Body).Decode(&e); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid json"})
		return
	}
	if strings.TrimSpace(e.Code) == "" || strings.TrimSpace(e.URL) == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "code and url are required"})
		return
	}
	if err := s.store.UpsertEmote(r.Context(), slug, e); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"emote": e})
}

func (s *Server) handleMintToken(w http.ResponseWriter, r *http.Request) {
	u, err := s.authenticateBearer(r)
	if err != nil {
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": err.Error()})
		return
	}
	var body struct {
		Label string `json:"label"`
	}
	_ = json.NewDecoder(r.Body).Decode(&body)

	role := u.Role
	if role == "" {
		role = "member"
	}
	token, err := s.store.CreateAPIToken(r.Context(), store.APITokenRecord{
		UserID:      u.ID,
		Username:    u.Username,
		DisplayName: u.DisplayName,
		Role:        role,
		IsAdmin:     u.IsAdmin,
	}, body.Label)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusCreated, map[string]any{
		"token": token,
		"user":  u,
	})
}

func (s *Server) handleWS(w http.ResponseWriter, r *http.Request) {
	opts := &websocket.AcceptOptions{
		InsecureSkipVerify: len(s.cfg.AllowedOrigins) == 0,
		OriginPatterns:     s.cfg.AllowedOrigins,
	}
	conn, err := websocket.Accept(w, r, opts)
	if err != nil {
		s.logger.Debug("ws accept failed", "err", err)
		return
	}
	conn.SetReadLimit(64 * 1024)

	client := hub.NewClient()
	ctx, cancel := context.WithCancel(r.Context())
	defer func() {
		cancel()
		s.hub.Disconnect(context.Background(), client)
		_ = conn.Close(websocket.StatusNormalClosure, "")
	}()

	// Writer goroutine drains client.Send to the socket.
	go func() {
		for {
			select {
			case <-ctx.Done():
				return
			case msg, ok := <-client.Send:
				if !ok {
					return
				}
				wctx, wcancel := context.WithTimeout(ctx, 10*time.Second)
				err := conn.Write(wctx, websocket.MessageText, msg)
				wcancel()
				if err != nil {
					cancel()
					return
				}
			}
		}
	}()

	// Reader loop processes incoming JSON frames.
	for {
		_, data, err := conn.Read(ctx)
		if err != nil {
			return
		}
		var env protocol.Envelope
		if err := json.Unmarshal(data, &env); err != nil {
			client.TrySend(protocol.MustMarshalEnvelope("error", "", protocol.ErrorPayload{
				Code: "bad_request", Message: "Invalid JSON frame.",
			}))
			continue
		}
		s.dispatchFrame(ctx, client, env)
	}
}

func (s *Server) dispatchFrame(ctx context.Context, client *hub.Client, env protocol.Envelope) {
	switch env.Type {
	case "auth":
		var req protocol.AuthRequest
		if err := json.Unmarshal(env.Data, &req); err != nil {
			client.TrySend(protocol.MustMarshalEnvelope("error", env.ReqID, protocol.ErrorPayload{Code: "bad_request", Message: "Invalid auth payload."}))
			return
		}
		u, err := s.auth.VerifyJWT(ctx, req.Token)
		if err != nil {
			client.TrySend(protocol.MustMarshalEnvelope("error", env.ReqID, protocol.ErrorPayload{Code: "unauthorized", Message: err.Error()}))
			return
		}
		client.SetUser(u)
		client.TrySend(protocol.MustMarshalEnvelope("auth_ok", env.ReqID, protocol.AuthOKPayload{User: u}))

	case "auth_guest":
		var req protocol.AuthGuestRequest
		if err := json.Unmarshal(env.Data, &req); err != nil {
			client.TrySend(protocol.MustMarshalEnvelope("error", env.ReqID, protocol.ErrorPayload{Code: "bad_request", Message: "Invalid guest payload."}))
			return
		}
		u, guestToken, err := s.auth.AuthGuest(req.Nickname, req.GuestToken)
		if err != nil {
			client.TrySend(protocol.MustMarshalEnvelope("error", env.ReqID, protocol.ErrorPayload{Code: "bad_request", Message: err.Error()}))
			return
		}
		client.SetUser(u)
		client.TrySend(protocol.MustMarshalEnvelope("auth_ok", env.ReqID, protocol.AuthOKPayload{User: u, GuestToken: guestToken}))

	case "auth_token":
		var req protocol.AuthTokenRequest
		if err := json.Unmarshal(env.Data, &req); err != nil {
			client.TrySend(protocol.MustMarshalEnvelope("error", env.ReqID, protocol.ErrorPayload{Code: "bad_request", Message: "Invalid token payload."}))
			return
		}
		u, err := s.auth.VerifyPersonalToken(ctx, req.Token, req.DisplayName)
		if err != nil {
			client.TrySend(protocol.MustMarshalEnvelope("error", env.ReqID, protocol.ErrorPayload{Code: "unauthorized", Message: err.Error()}))
			return
		}
		client.SetUser(u)
		client.TrySend(protocol.MustMarshalEnvelope("auth_ok", env.ReqID, protocol.AuthOKPayload{User: u}))

	case "join":
		var req protocol.RoomRequest
		if err := json.Unmarshal(env.Data, &req); err != nil {
			client.TrySend(protocol.MustMarshalEnvelope("error", env.ReqID, protocol.ErrorPayload{Code: "bad_request", Message: "Invalid join payload."}))
			return
		}
		joined, err := s.hub.Join(ctx, client, req.Room)
		if err != nil {
			client.TrySend(protocol.MustMarshalEnvelope("error", env.ReqID, protocol.ErrorPayload{Code: "internal_error", Message: err.Error()}))
			return
		}
		client.TrySend(protocol.MustMarshalEnvelope("room_joined", env.ReqID, joined))

	case "leave":
		var req protocol.RoomRequest
		_ = json.Unmarshal(env.Data, &req)
		s.hub.Leave(ctx, client, req.Room)

	case "send_message":
		var req protocol.SendMessageRequest
		if err := json.Unmarshal(env.Data, &req); err != nil {
			client.TrySend(protocol.MustMarshalEnvelope("error", env.ReqID, protocol.ErrorPayload{Code: "bad_request", Message: "Invalid message payload."}))
			return
		}
		s.hub.HandleSendMessage(ctx, client, env.ReqID, req)

	case "moderate":
		var req protocol.ModerateRequest
		if err := json.Unmarshal(env.Data, &req); err != nil {
			client.TrySend(protocol.MustMarshalEnvelope("error", env.ReqID, protocol.ErrorPayload{Code: "bad_request", Message: "Invalid moderation payload."}))
			return
		}
		s.hub.HandleModerate(ctx, client, env.ReqID, req)

	case "ping":
		var req protocol.PingRequest
		_ = json.Unmarshal(env.Data, &req)
		client.TrySend(protocol.MustMarshalEnvelope("pong", env.ReqID, req))

	default:
		client.TrySend(protocol.MustMarshalEnvelope("error", env.ReqID, protocol.ErrorPayload{
			Code: "bad_request", Message: "Unknown frame type: " + env.Type,
		}))
	}
}

func (s *Server) authenticateBearer(r *http.Request) (protocol.User, error) {
	hdr := strings.TrimSpace(r.Header.Get("Authorization"))
	token := strings.TrimSpace(strings.TrimPrefix(hdr, "Bearer "))
	if token == "" {
		return protocol.User{}, http.ErrNoCookie
	}
	if u, err := s.auth.VerifyPersonalToken(r.Context(), token, ""); err == nil {
		return u, nil
	}
	return s.auth.VerifyJWT(r.Context(), token)
}

func (s *Server) setCORS(w http.ResponseWriter, r *http.Request) {
	origin := r.Header.Get("Origin")
	if len(s.cfg.AllowedOrigins) == 0 {
		w.Header().Set("Access-Control-Allow-Origin", "*")
	} else if origin != "" {
		for _, allowed := range s.cfg.AllowedOrigins {
			if allowed == "*" || allowed == origin {
				w.Header().Set("Access-Control-Allow-Origin", origin)
				break
			}
		}
	}
	w.Header().Set("Access-Control-Allow-Headers", "Authorization, Content-Type")
	w.Header().Set("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
}

func writeJSON(w http.ResponseWriter, status int, payload any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(payload)
}
