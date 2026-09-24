FROM rust:1.98

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
  --mount=type=cache,target=/var/lib/apt,sharing=locked \
  apt update && apt-get upgrade -y && apt install -y \
  libwayland-dev libwayland-egl-backend-dev libxkbcommon-dev libgbm-dev  libinput-dev libudev-dev libseat-dev libgbm-dev libpixman-1-dev libdisplay-info-dev xwayland