FROM alpine:3.22

ARG TARGET
ARG BINARY

WORKDIR /app

COPY target/${TARGET}/release/${BINARY} /app/bin

RUN chmod +x /app/bin

USER 1001:1001
ENTRYPOINT ["/app/bin"]
