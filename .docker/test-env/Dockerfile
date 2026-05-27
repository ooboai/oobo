FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    openssh-server \
    git \
    curl \
    ca-certificates \
    sudo \
    vim \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -s /bin/bash dev \
    && echo "dev:dev" | chpasswd \
    && usermod -aG sudo dev \
    && echo "dev ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

RUN mkdir /run/sshd

RUN su - dev -c 'git config --global user.name "Test User" && git config --global user.email "dev@test.local"'

EXPOSE 22

CMD ["/usr/sbin/sshd", "-D"]
