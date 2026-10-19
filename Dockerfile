FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p eas-server

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/stalwart-activesync /usr/local/bin/stalwart-activesync
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/stalwart-activesync"]
