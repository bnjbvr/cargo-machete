FROM rust:slim AS build

COPY . /src
RUN cargo install --path /src --locked

FROM debian:stable-slim

COPY --from=build /usr/local/cargo/bin/cargo-machete /usr/local/bin

WORKDIR /src
ENTRYPOINT ["cargo-machete"]