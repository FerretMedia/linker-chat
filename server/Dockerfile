FROM golang:1.24-alpine AS build
WORKDIR /src
COPY go.mod go.sum* ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 GOOS=linux go build -ldflags="-s -w" -o /out/linker-chat-server ./cmd/linker-chat-server

FROM alpine:3.21
RUN apk add --no-cache ca-certificates
WORKDIR /data
COPY --from=build /out/linker-chat-server /usr/local/bin/linker-chat-server
EXPOSE 8080
ENV LINKER_CHAT_ADDR=":8080"
ENV LINKER_CHAT_DB="/data/linker-chat.db"
ENTRYPOINT ["linker-chat-server"]
