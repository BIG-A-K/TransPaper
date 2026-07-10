FROM ubuntu:latest

ARG UID
ARG USER
ARG GROUP
ARG GID

ENV TZ=Asia/Tokyo
RUN ln -snf /usr/share/zoneinfo/$TZ /etc/localtime && echo $TZ > /etc/timezone

USER root

RUN rm -f /etc/apt/apt.conf.d/docker-clean \
    && echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && \
    apt-get upgrade -y&&\
    apt-get install -y \
    ca-certificates \
    curl \
    tmux \
    build-essential \
    fonts*noto* \
    git \
    git-lfs \
    jq \
    gpg \
    iproute2 \
    iputils-ping \
    libssl-dev \
    net-tools \
    imagemagick \
    nfs-common \
    openssh-server \
    python3-dev \
    python3-opencv \
    python3-pip \
    python3-venv \
    sudo \
    zsh\
    vim \
    wget \
    x11-apps \
    xauth \
    fontconfig\
    libfontconfig1-dev clang libclang-dev unzip zip tar


# install Rust (stable) via rustup
# rustup is installed into a shared CARGO_HOME/RUSTUP_HOME (matching PATH) so
# the non-root ${USER} can also run cargo/rustc. Permissions are opened up so
# the user can write to the registry cache etc.
ENV CARGO_HOME=/usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/usr/local/rustup sh -s -- -y --profile minimal \
    && chmod -R a+rwX /usr/local/cargo /usr/local/rustup
ENV PATH="/usr/local/cargo/bin:${PATH}"

# install uv
RUN curl -LsSf https://astral.sh/uv/install.sh | env UV_UNMANAGED_INSTALL="/usr/local/bin" sh
ENV UV_LINK_MODE=copy

# install Node.js (LTS) + npm
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - && \
    apt-get install -y nodejs

# install codex
RUN npm install -g @openai/codex

RUN deluser ubuntu

RUN addgroup --gid ${GID} ${GROUP}
RUN useradd -ms /bin/zsh -u ${UID} -g ${GID} -G sudo ${USER} &&\
    echo ${USER}:${USER} | chpasswd

USER ${USER}
WORKDIR /home/${USER}
RUN mkdir -p /home/${USER}/.config


# install claude code
RUN curl -fsSL https://claude.ai/install.sh | bash

CMD ["/bin/zsh"]
