FROM alpine:3.22.8

# Pull latest security fixes (incl. ca-certificates-bundle) at build time.
RUN apk upgrade --no-cache

ARG TARGET
ARG BINARY

WORKDIR /app

COPY target/${TARGET}/release/${BINARY} /app/bin

RUN chmod +x /app/bin

USER 1001:1001
ENTRYPOINT ["/app/bin"]
