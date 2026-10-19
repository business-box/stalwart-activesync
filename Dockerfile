FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p eas-server && mkdir -p /data

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/stalwart-activesync /usr/local/bin/stalwart-activesync
COPY --from=build --chown=65532:65532 /data /data
ENV EAS_DB_URL=sqlite:///data/eas.db
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/stalwart-activesync"]
