package auth

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/hmac"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"hash/fnv"
	"math/big"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/FerretMedia/linker-chat/server/internal/config"
	"github.com/FerretMedia/linker-chat/server/internal/protocol"
	"github.com/FerretMedia/linker-chat/server/internal/store"
)

var colorPalette = []string{
	"#FF6B6B", "#4ECDC4", "#FFD166", "#A78BFA", "#F472B6",
	"#38BDF8", "#34D399", "#FB923C", "#FACC15", "#C084FC",
}

// Verifier handles Linker Clerk JWKS verification, HMAC dev tokens, guests, and personal API tokens.
type Verifier struct {
	cfg        config.Config
	store      *store.Store
	httpClient *http.Client

	mu        sync.RWMutex
	keys      map[string]any
	fetchedAt time.Time
}

type jwksResponse struct {
	Keys []jwkKey `json:"keys"`
}

type jwkKey struct {
	Kty string `json:"kty"`
	Kid string `json:"kid"`
	Use string `json:"use"`
	Alg string `json:"alg"`
	N   string `json:"n"`
	E   string `json:"e"`
	Crv string `json:"crv"`
	X   string `json:"x"`
	Y   string `json:"y"`
}

// NewVerifier creates an auth verifier backed by config and SQLite store.
func NewVerifier(cfg config.Config, st *store.Store) *Verifier {
	return &Verifier{
		cfg:   cfg,
		store: st,
		httpClient: &http.Client{
			Timeout: 8 * time.Second,
		},
		keys: make(map[string]any),
	}
}

// VerifyJWT validates a Linker Clerk RS256/ES256 JWT (via JWKS) or an HS256 JWT (if JWTSecret is set).
func (v *Verifier) VerifyJWT(ctx context.Context, rawToken string) (protocol.User, error) {
	rawToken = strings.TrimSpace(rawToken)
	if rawToken == "" {
		return protocol.User{}, errors.New("missing token")
	}

	claims := jwt.MapClaims{}
	parserOpts := []jwt.ParserOption{
		jwt.WithValidMethods([]string{"RS256", "RS384", "RS512", "ES256", "ES384", "HS256"}),
	}
	if v.cfg.ClerkIssuer != "" {
		parserOpts = append(parserOpts, jwt.WithIssuer(v.cfg.ClerkIssuer))
	}
	if v.cfg.ClerkAudience != "" {
		parserOpts = append(parserOpts, jwt.WithAudience(v.cfg.ClerkAudience))
	}

	tok, err := jwt.ParseWithClaims(rawToken, claims, func(token *jwt.Token) (any, error) {
		alg := token.Method.Alg()
		if strings.HasPrefix(alg, "HS") {
			if v.cfg.JWTSecret == "" {
				return nil, errors.New("HMAC JWT verification is not enabled")
			}
			return []byte(v.cfg.JWTSecret), nil
		}
		if v.cfg.ClerkJWKSURL == "" {
			return nil, errors.New("Clerk JWKS URL is not configured")
		}
		kid, _ := token.Header["kid"].(string)
		return v.lookupJWKSKey(ctx, kid)
	}, parserOpts...)
	if err != nil || !tok.Valid {
		return protocol.User{}, fmt.Errorf("invalid jwt: %w", err)
	}

	sub := claimString(claims, "sub")
	if sub == "" {
		return protocol.User{}, errors.New("jwt missing sub claim")
	}
	username := firstNonEmpty(
		claimString(claims, "username"),
		claimString(claims, "preferred_username"),
		claimString(claims, "nickname"),
		emailPrefix(claimString(claims, "email")),
		"user_"+shortHash(sub),
	)
	displayName := firstNonEmpty(
		claimString(claims, "display_name"),
		claimString(claims, "name"),
		username,
	)
	avatar := firstNonEmpty(
		claimString(claims, "image_url"),
		claimString(claims, "avatar_url"),
		claimString(claims, "picture"),
	)
	color := claimString(claims, "color")
	if color == "" {
		color = ColorForSeed(sub)
	}
	role := claimString(claims, "role")
	isAdmin := role == "admin" || role == "broadcaster"

	return protocol.User{
		ID:          "clerk:" + sub,
		Username:    sanitizeHandle(username),
		DisplayName: strings.TrimSpace(displayName),
		AvatarURL:   avatar,
		Color:       color,
		Role:        role,
		IsGuest:     false,
		IsAdmin:     isAdmin,
	}, nil
}

// AuthGuest authenticates or resumes a guest identity using an HMAC-signed token.
func (v *Verifier) AuthGuest(nickname, guestToken string) (protocol.User, string, error) {
	if guestToken != "" {
		if user, ok := v.verifyGuestToken(guestToken); ok {
			if cleaned := sanitizeNickname(nickname); cleaned != "" {
				user.DisplayName = cleaned
				user.Username = sanitizeHandle(cleaned)
			}
			return user, guestToken, nil
		}
	}

	cleaned := sanitizeNickname(nickname)
	if cleaned == "" {
		return protocol.User{}, "", errors.New("nickname must be 2-24 characters")
	}

	buf := make([]byte, 6)
	_, _ = rand.Read(buf)
	guestID := "guest:" + hex.EncodeToString(buf)
	user := protocol.User{
		ID:          guestID,
		Username:    sanitizeHandle(cleaned),
		DisplayName: cleaned,
		Color:       ColorForSeed(guestID),
		Role:        "guest",
		IsGuest:     true,
	}
	tok := v.signGuestToken(user)
	return user, tok, nil
}

// VerifyPersonalToken checks either the server's AdminToken or a SQLite-stored personal API token.
func (v *Verifier) VerifyPersonalToken(ctx context.Context, rawToken, displayName string) (protocol.User, error) {
	rawToken = strings.TrimSpace(rawToken)
	if rawToken == "" {
		return protocol.User{}, errors.New("missing token")
	}

	if v.cfg.AdminToken != "" && subtle.ConstantTimeCompare([]byte(rawToken), []byte(v.cfg.AdminToken)) == 1 {
		name := strings.TrimSpace(displayName)
		if name == "" {
			name = "Broadcaster"
		}
		return protocol.User{
			ID:          "admin:broadcaster",
			Username:    sanitizeHandle(name),
			DisplayName: name,
			Color:       "#A78BFA",
			Role:        "broadcaster",
			IsGuest:     false,
			IsAdmin:     true,
		}, nil
	}

	rec, err := v.store.ValidateAPIToken(ctx, rawToken)
	if err != nil {
		return protocol.User{}, err
	}
	if rec == nil {
		return protocol.User{}, errors.New("invalid api token")
	}
	name := rec.DisplayName
	if strings.TrimSpace(displayName) != "" {
		name = strings.TrimSpace(displayName)
	}
	return protocol.User{
		ID:          rec.UserID,
		Username:    rec.Username,
		DisplayName: name,
		Color:       ColorForSeed(rec.UserID),
		Role:        rec.Role,
		IsGuest:     false,
		IsAdmin:     rec.IsAdmin,
	}, nil
}

func (v *Verifier) lookupJWKSKey(ctx context.Context, kid string) (any, error) {
	v.mu.RLock()
	key, ok := v.keys[kid]
	fresh := time.Since(v.fetchedAt) < 15*time.Minute
	if !ok && kid == "" && len(v.keys) == 1 && fresh {
		for _, only := range v.keys {
			key = only
			ok = true
		}
	}
	v.mu.RUnlock()
	if ok && fresh {
		return key, nil
	}

	if err := v.refreshJWKS(ctx); err != nil {
		return nil, err
	}

	v.mu.RLock()
	defer v.mu.RUnlock()
	if kid != "" {
		if k, exists := v.keys[kid]; exists {
			return k, nil
		}
		return nil, fmt.Errorf("jwk kid %q not found", kid)
	}
	for _, only := range v.keys {
		return only, nil
	}
	return nil, errors.New("jwks contained no usable keys")
}

func (v *Verifier) refreshJWKS(ctx context.Context) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, v.cfg.ClerkJWKSURL, nil)
	if err != nil {
		return err
	}
	resp, err := v.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("fetch jwks: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("jwks returned HTTP %d", resp.StatusCode)
	}

	var body jwksResponse
	if err := json.NewDecoder(resp.Body).Decode(&body); err != nil {
		return fmt.Errorf("decode jwks: %w", err)
	}

	parsed := make(map[string]any, len(body.Keys))
	for _, k := range body.Keys {
		pub, err := parseJWK(k)
		if err != nil {
			continue
		}
		parsed[k.Kid] = pub
	}

	v.mu.Lock()
	v.keys = parsed
	v.fetchedAt = time.Now()
	v.mu.Unlock()
	return nil
}

func parseJWK(k jwkKey) (any, error) {
	switch k.Kty {
	case "RSA":
		nBytes, err := base64.RawURLEncoding.DecodeString(k.N)
		if err != nil {
			return nil, err
		}
		eBytes, err := base64.RawURLEncoding.DecodeString(k.E)
		if err != nil {
			return nil, err
		}
		e := 0
		for _, b := range eBytes {
			e = e<<8 + int(b)
		}
		return &rsa.PublicKey{
			N: new(big.Int).SetBytes(nBytes),
			E: e,
		}, nil
	case "EC":
		var curve elliptic.Curve
		switch k.Crv {
		case "P-256":
			curve = elliptic.P256()
		case "P-384":
			curve = elliptic.P384()
		default:
			return nil, fmt.Errorf("unsupported curve %s", k.Crv)
		}
		xBytes, err := base64.RawURLEncoding.DecodeString(k.X)
		if err != nil {
			return nil, err
		}
		yBytes, err := base64.RawURLEncoding.DecodeString(k.Y)
		if err != nil {
			return nil, err
		}
		return &ecdsa.PublicKey{
			Curve: curve,
			X:     new(big.Int).SetBytes(xBytes),
			Y:     new(big.Int).SetBytes(yBytes),
		}, nil
	default:
		return nil, fmt.Errorf("unsupported kty %s", k.Kty)
	}
}

func (v *Verifier) signGuestToken(u protocol.User) string {
	payload := base64.RawURLEncoding.EncodeToString([]byte(u.ID + "|" + u.DisplayName + "|" + u.Color))
	mac := hmac.New(sha256.New, []byte(v.cfg.GuestSecret))
	mac.Write([]byte(payload))
	sig := base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
	return payload + "." + sig
}

func (v *Verifier) verifyGuestToken(token string) (protocol.User, bool) {
	parts := strings.Split(token, ".")
	if len(parts) != 2 {
		return protocol.User{}, false
	}
	mac := hmac.New(sha256.New, []byte(v.cfg.GuestSecret))
	mac.Write([]byte(parts[0]))
	expected := base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
	if subtle.ConstantTimeCompare([]byte(parts[1]), []byte(expected)) != 1 {
		return protocol.User{}, false
	}
	raw, err := base64.RawURLEncoding.DecodeString(parts[0])
	if err != nil {
		return protocol.User{}, false
	}
	fields := strings.SplitN(string(raw), "|", 3)
	if len(fields) != 3 {
		return protocol.User{}, false
	}
	return protocol.User{
		ID:          fields[0],
		Username:    sanitizeHandle(fields[1]),
		DisplayName: fields[1],
		Color:       fields[2],
		Role:        "guest",
		IsGuest:     true,
	}, true
}

// ColorForSeed picks a consistent readable username color.
func ColorForSeed(seed string) string {
	h := fnv.New32a()
	_, _ = h.Write([]byte(seed))
	return colorPalette[int(h.Sum32())%len(colorPalette)]
}

func claimString(m jwt.MapClaims, key string) string {
	if val, ok := m[key]; ok {
		if s, ok := val.(string); ok {
			return strings.TrimSpace(s)
		}
	}
	return ""
}

func firstNonEmpty(vals ...string) string {
	for _, s := range vals {
		if strings.TrimSpace(s) != "" {
			return strings.TrimSpace(s)
		}
	}
	return ""
}

func emailPrefix(email string) string {
	if i := strings.IndexByte(email, '@'); i > 0 {
		return email[:i]
	}
	return ""
}

func shortHash(s string) string {
	sum := sha256.Sum256([]byte(s))
	return hex.EncodeToString(sum[:3])
}

func sanitizeNickname(raw string) string {
	cleaned := strings.TrimSpace(raw)
	if len(cleaned) < 2 || len(cleaned) > 24 {
		return ""
	}
	return cleaned
}

func sanitizeHandle(raw string) string {
	raw = strings.ToLower(strings.TrimSpace(raw))
	var b strings.Builder
	for _, r := range raw {
		if (r >= 'a' && r <= 'z') || (r >= '0' && r <= '9') || r == '_' || r == '-' {
			b.WriteRune(r)
		} else if r == ' ' {
			b.WriteRune('_')
		}
	}
	out := b.String()
	if out == "" {
		return "user"
	}
	return out
}
