# syntax=docker/dockerfile:1
FROM rust:alpine@sha256:7e9e2d068a107a2adafd19930011178d14a78326053cd4c3a9f7ad3fd049557d AS build
RUN apk add --no-cache build-base pkgconf dbus-dev file binutils
WORKDIR /source
COPY Cargo.toml Cargo.lock ./
COPY src ./src
ARG SSHM_CONFIG_FINGERPRINT
RUN --mount=type=secret,id=google_client_id,required=true \
    --mount=type=secret,id=google_client_secret,required=true \
    test -n "$SSHM_CONFIG_FINGERPRINT" && \
    SSHM_GOOGLE_CLIENT_ID="$(cat /run/secrets/google_client_id)" \
    SSHM_GOOGLE_CLIENT_SECRET="$(cat /run/secrets/google_client_secret)" \
    cargo build --locked --release --target x86_64-unknown-linux-musl --bin sshm
RUN mkdir /output && \
    cp target/x86_64-unknown-linux-musl/release/sshm /output/sshm && \
    file /output/sshm && \
    ! readelf -l /output/sshm | grep -q INTERP
RUN /output/sshm --version && /output/sshm --help >/dev/null
FROM scratch AS artifact
COPY --from=build /output/sshm /sshm
