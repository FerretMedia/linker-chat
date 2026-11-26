package httpapi_test

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/rsa"
	"encoding/base64"
	"encoding/json"
	"math/big"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/golang-jwt/jwt/v5"
	"github.com/FerretMedia/linker-chat/server/internal/auth"
	"github.com/FerretMedia/linker-chat/server/internal/config"
	"github.com/FerretMedia/linker-chat/server/internal/httpapi"
	"github.com/FerretMedia/linker-chat/server/internal/hub"
	"github.com/FerretMedia/linker-chat/server/internal/protocol"
	"github.com/FerretMedia/linker-chat/server/internal/store"
)

func TestClerkJWKSChatAndModeration(t *testing.T) {
	// 1. Spin up a mock Linker Clerk JWKS server with a real RSA keypair.
	privKey, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatalf("generate rsa key: %v", err)
	}
	const kid = "linker-clerk-key-1"
	jwksSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		eBytes := big.NewInt(int64(privKey.PublicKey.E)).Bytes()
		resp := map[string]any{
			"keys": []map[string]any{{
				"kty": "RSA",
				"kid": kid,
				"alg": "RS256",
				"use": "sig",
				"n":   base64.RawURLEncoding.EncodeToString(privKey.PublicKey.N.Bytes()),
				"e":   base64.RawURLEncoding.EncodeToString(eBytes),
			}},
		}
		_ = json.NewEncoder(w).Encode(resp)
	}))
	defer jwksSrv.Close()

	// 2. Start Linker-chat server with in-memory SQLite.
	cfg := config.Config{
		DBPath:           ":memory:",
		ClerkJWKSURL:     jwksSrv.URL,
		ClerkIssuer:      "https://clerk.linker.example",
		GuestSecret:      "test-guest-secret",
		AdminToken:       "super-admin-token",
		AllowGuests:      true,
		MaxMessageLength: 500,
		HistoryLimit:     50,
	}
	st, err := store.Open(cfg.DBPath, cfg.AllowGuests)
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer st.Close()

	ver := auth.NewVerifier(cfg, st)
	chatHub := hub.New(cfg, st)
	api := httpapi.New(cfg, st, ver, chatHub, nil)
	ts := httptest.NewServer(api)
	defer ts.Close()

	wsURL := "ws" + strings.TrimPrefix(ts.URL, "http") + "/ws"
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// 3. Connect Broadcaster via personal AdminToken (GPUI desktop flow).
	modConn, _, err := websocket.Dial(ctx, wsURL, nil)
	if err != nil {
		t.Fatalf("dial mod: %v", err)
	}
	defer modConn.Close(websocket.StatusNormalClosure, "")

	sendFrame(t, ctx, modConn, "auth_token", "r1", protocol.AuthTokenRequest{
		Token: "super-admin-token", DisplayName: "Streamer",
	})
	authEnv := readUntilType(t, ctx, modConn, "auth_ok")
	var modAuth protocol.AuthOKPayload
	_ = json.Unmarshal(authEnv.Data, &modAuth)
	if !modAuth.User.IsAdmin || modAuth.User.DisplayName != "Streamer" {
		t.Fatalf("unexpected mod auth: %+v", modAuth.User)
	}
	sendFrame(t, ctx, modConn, "join", "r2", protocol.RoomRequest{Room: "main"})
	readUntilType(t, ctx, modConn, "room_joined")

	// 4. Sign a Linker Clerk JWT for a viewer and connect.
	token := jwt.NewWithClaims(jwt.SigningMethodRS256, jwt.MapClaims{
		"sub":      "user_clerk_123",
		"iss":      "https://clerk.linker.example",
		"username": "ferret_dev",
		"name":     "Ferret Dev",
		"exp":      time.Now().Add(time.Hour).Unix(),
	})
	token.Header["kid"] = kid
	signedJWT, err := token.SignedString(privKey)
	if err != nil {
		t.Fatalf("sign jwt: %v", err)
	}

	viewerConn, _, err := websocket.Dial(ctx, wsURL, nil)
	if err != nil {
		t.Fatalf("dial viewer: %v", err)
	}
	defer viewerConn.Close(websocket.StatusNormalClosure, "")

	sendFrame(t, ctx, viewerConn, "auth", "v1", protocol.AuthRequest{Token: signedJWT})
	vAuthEnv := readUntilType(t, ctx, viewerConn, "auth_ok")
	var viewerAuth protocol.AuthOKPayload
	_ = json.Unmarshal(vAuthEnv.Data, &viewerAuth)
	if viewerAuth.User.Username != "ferret_dev" || viewerAuth.User.IsGuest {
		t.Fatalf("unexpected clerk user: %+v", viewerAuth.User)
	}
	sendFrame(t, ctx, viewerConn, "join", "v2", protocol.RoomRequest{Room: "main"})
	readUntilType(t, ctx, viewerConn, "room_joined")

	// 5. Viewer sends a message and Broadcaster receives it live.
	sendFrame(t, ctx, viewerConn, "send_message", "v3", protocol.SendMessageRequest{
		Room: "main", Content: "Hello from Linker Clerk!",
	})
	msgEnv := readUntilType(t, ctx, modConn, "message")
	var msg protocol.ChatMessage
	_ = json.Unmarshal(msgEnv.Data, &msg)
	if msg.Content != "Hello from Linker Clerk!" || msg.User.Username != "ferret_dev" {
		t.Fatalf("unexpected message: %+v", msg)
	}

	// 6. Broadcaster deletes the message and times out viewer via slash command.
	sendFrame(t, ctx, modConn, "moderate", "m1", protocol.ModerateRequest{
		Room: "main", Action: "delete", MessageID: msg.ID,
	})
	readUntilType(t, ctx, viewerConn, "message_deleted")

	sendFrame(t, ctx, modConn, "send_message", "m2", protocol.SendMessageRequest{
		Room: "main", Content: "/timeout ferret_dev 30 chill out",
	})
	readUntilType(t, ctx, viewerConn, "user_moderated")

	// Viewer's next message should be rejected with timed_out error.
	sendFrame(t, ctx, viewerConn, "send_message", "v4", protocol.SendMessageRequest{
		Room: "main", Content: "Can I still talk?",
	})
	errEnv := readUntilType(t, ctx, viewerConn, "error")
	var errPayload protocol.ErrorPayload
	_ = json.Unmarshal(errEnv.Data, &errPayload)
	if errPayload.Code != "timed_out" {
		t.Fatalf("expected timed_out error, got %+v", errPayload)
	}

	// 7. Mint a personal desktop API token using the Clerk JWT via REST.
	req, _ := http.NewRequest(http.MethodPost, ts.URL+"/api/v1/tokens", bytes.NewBufferString(`{"label":"gpui-linux"}`))
	req.Header.Set("Authorization", "Bearer "+signedJWT)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("mint token: %v", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("expected 201 Created, got %d", resp.StatusCode)
	}
	var tokenResp struct {
		Token string `json:"token"`
	}
	_ = json.NewDecoder(resp.Body).Decode(&tokenResp)
	if !strings.HasPrefix(tokenResp.Token, "lct_") {
		t.Fatalf("expected lct_ token prefix, got %q", tokenResp.Token)
	}
}

func sendFrame(t *testing.T, ctx context.Context, conn *websocket.Conn, typ, reqID string, data any) {
	t.Helper()
	frame := protocol.MustMarshalEnvelope(typ, reqID, data)
	if err := conn.Write(ctx, websocket.MessageText, frame); err != nil {
		t.Fatalf("ws write %s: %v", typ, err)
	}
}

func readUntilType(t *testing.T, ctx context.Context, conn *websocket.Conn, want string) protocol.Envelope {
	t.Helper()
	for {
		_, raw, err := conn.Read(ctx)
		if err != nil {
			t.Fatalf("ws read waiting for %s: %v", want, err)
		}
		var env protocol.Envelope
		if err := json.Unmarshal(raw, &env); err != nil {
			t.Fatalf("unmarshal frame: %v", err)
		}
		if env.Type == want {
			return env
		}
	}
}
