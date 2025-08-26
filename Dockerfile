FROM rust:slim AS build

COPY . /src
RUN cargo install --path /src --locked
RUN apt update && apt -y install tini

FROM gcr.io/distroless/cc-debian13
COPY --from=build /usr/local/cargo/bin/cargo-machete /usr/local/bin/
COPY --from=build /usr/bin/tini-static /tini

WORKDIR /src
ENTRYPOINT ["/tini", "--", "/usr/local/bin/cargo-machete"]
