FROM rust:1.92.0 AS build
WORKDIR /usr/src/subcast
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src
COPY . .
RUN touch src/main.rs && cargo build --release --frozen

FROM nvidia/cuda:13.1.0-base-ubuntu24.04 AS publish
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg && rm -rf /var/lib/apt/lists/*
ENV NVIDIA_VISIBLE_DEVICES=all
ENV NVIDIA_DRIVER_CAPABILITIES=compute,video,utility
COPY --from=builder /usr/src/subcast/target/release/subcast /usr/local/bin/subcast
ENTRYPOINT [ "subcast" ]