FROM ubuntu:focal

RUN apt-get update -y && apt-get install -y curl gnupg python3.8

RUN set -eux; \
    curl https://pkg.ebbflow.io/live/ubuntu/focal.gpg | apt-key add -; \
    curl https://pkg.ebbflow.io/live/ubuntu/focal.list | tee /etc/apt/sources.list.d/ebbflow.list; \
    apt update && apt install ebbflow;

RUN useradd -ms /bin/bash toby
COPY entrypoint.sh /entrypoint.sh
USER toby
WORKDIR /home/toby
ENTRYPOINT ["/entrypoint.sh"]