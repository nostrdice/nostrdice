FROM ubuntu:22.04

ARG BINARY=target/debug/nostr-dice

RUN apt-get update && \
    apt-get install ca-certificates -y

USER 1000

COPY $BINARY /usr/bin/nostr-dice

ENTRYPOINT ["/usr/bin/nostr-dice"]