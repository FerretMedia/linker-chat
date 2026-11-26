package config

import (
	"crypto/rand"
	"encoding/hex"
	"flag"
	"os"
	"strconv"
	"strings"
)

// Config holds runtime configuration for the Linker-chat server.
type Config struct {
	Addr             string
	DBPath           string
	ClerkJWKSURL     string
	ClerkIssuer      string
	ClerkAudience    string
	JWTSecret        string
	GuestSecret      string
	AdminToken       string
	AllowGuests      bool
	AllowedOrigins   []string
	MaxMessageLength int
	HistoryLimit     int
}

// Load reads configuration from environment variables and optional CLI flags.
func Load() Config {
	cfg := Config{
		Addr:             envOr("LINKER_CHAT_ADDR", ":8080"),
		DBPath:           envOr("LINKER_CHAT_DB", "./linker-chat.db"),
		ClerkJWKSURL:     os.Getenv("LINKER_CHAT_CLERK_JWKS_URL"),
		ClerkIssuer:      os.Getenv("LINKER_CHAT_CLERK_ISSUER"),
		ClerkAudience:    os.Getenv("LINKER_CHAT_CLERK_AUDIENCE"),
		JWTSecret:        os.Getenv("LINKER_CHAT_JWT_SECRET"),
		GuestSecret:      os.Getenv("LINKER_CHAT_GUEST_SECRET"),
		AdminToken:       os.Getenv("LINKER_CHAT_ADMIN_TOKEN"),
		AllowGuests:      envBoolOr("LINKER_CHAT_ALLOW_GUESTS", true),
		AllowedOrigins:   splitCSV(os.Getenv("LINKER_CHAT_ALLOWED_ORIGINS")),
		MaxMessageLength: envIntOr("LINKER_CHAT_MAX_MESSAGE_LENGTH", 500),
		HistoryLimit:     envIntOr("LINKER_CHAT_HISTORY_LIMIT", 50),
	}

	flag.StringVar(&cfg.Addr, "addr", cfg.Addr, "HTTP/WebSocket listen address")
	flag.StringVar(&cfg.DBPath, "db", cfg.DBPath, "SQLite database file path")
	flag.StringVar(&cfg.ClerkJWKSURL, "clerk-jwks", cfg.ClerkJWKSURL, "Clerk OIDC JWKS URL")
	flag.StringVar(&cfg.ClerkIssuer, "clerk-issuer", cfg.ClerkIssuer, "Expected JWT issuer")
	flag.StringVar(&cfg.AdminToken, "admin-token", cfg.AdminToken, "Broadcaster/admin personal API token")
	flag.BoolVar(&cfg.AllowGuests, "allow-guests", cfg.AllowGuests, "Default guest chatting setting for new rooms")
	flag.Parse()

	if cfg.GuestSecret == "" {
		buf := make([]byte, 32)
		_, _ = rand.Read(buf)
		cfg.GuestSecret = hex.EncodeToString(buf)
	}
	return cfg
}

func envOr(key, fallback string) string {
	if v := strings.TrimSpace(os.Getenv(key)); v != "" {
		return v
	}
	return fallback
}

func envBoolOr(key string, fallback bool) bool {
	v := strings.TrimSpace(os.Getenv(key))
	if v == "" {
		return fallback
	}
	b, err := strconv.ParseBool(v)
	if err != nil {
		return fallback
	}
	return b
}

func envIntOr(key string, fallback int) int {
	v := strings.TrimSpace(os.Getenv(key))
	if v == "" {
		return fallback
	}
	n, err := strconv.Atoi(v)
	if err != nil {
		return fallback
	}
	return n
}

func splitCSV(raw string) []string {
	if strings.TrimSpace(raw) == "" {
		return nil
	}
	parts := strings.Split(raw, ",")
	out := make([]string, 0, len(parts))
	for _, p := range parts {
		if t := strings.TrimSpace(p); t != "" {
			out = append(out, t)
		}
	}
	return out
}
