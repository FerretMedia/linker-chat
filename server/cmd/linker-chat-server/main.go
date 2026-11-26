package main

import (
	"context"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/FerretMedia/linker-chat/server/internal/auth"
	"github.com/FerretMedia/linker-chat/server/internal/config"
	"github.com/FerretMedia/linker-chat/server/internal/httpapi"
	"github.com/FerretMedia/linker-chat/server/internal/hub"
	"github.com/FerretMedia/linker-chat/server/internal/store"
)

func main() {
	logger := slog.New(slog.NewTextHandler(os.Stdout, &slog.HandlerOptions{Level: slog.LevelInfo}))
	cfg := config.Load()

	st, err := store.Open(cfg.DBPath, cfg.AllowGuests)
	if err != nil {
		logger.Error("failed to open sqlite store", "err", err)
		os.Exit(1)
	}
	defer st.Close()

	ver := auth.NewVerifier(cfg, st)
	chatHub := hub.New(cfg, st)
	api := httpapi.New(cfg, st, ver, chatHub, logger)

	srv := &http.Server{
		Addr:              cfg.Addr,
		Handler:           api,
		ReadHeaderTimeout: 10 * time.Second,
	}

	go func() {
		logger.Info("Linker-chat server listening",
			"addr", cfg.Addr,
			"db", cfg.DBPath,
			"clerk_jwks_configured", cfg.ClerkJWKSURL != "",
			"allow_guests", cfg.AllowGuests,
		)
		if err := srv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			logger.Error("server crashed", "err", err)
			os.Exit(1)
		}
	}()

	stop := make(chan os.Signal, 1)
	signal.Notify(stop, syscall.SIGINT, syscall.SIGTERM)
	<-stop

	logger.Info("shutting down Linker-chat server")
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = srv.Shutdown(ctx)
}
