ARG BUILDPLATFORM
FROM --platform=$BUILDPLATFORM ubuntu:24.04

ARG TARGETPLATFORM

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    cmake \
    gcc-14 \
    g++-14 \
    libssl-dev \
    ninja-build \
    python3 \
    && rm -rf /var/lib/apt/lists/*

ENV CC=gcc-14
ENV CXX=g++-14

WORKDIR /src

COPY . .

RUN cmake -G Ninja -B build -DCMAKE_BUILD_TYPE=Release && cmake --build build -j$(nproc)

# Verify the binary runs
RUN ./build/exv version

# Smoke test
RUN ./build/exv help
