FROM ubuntu:22.04

ARG BINARY=target/debug/lnurl-server

RUN apt-get update && \
    apt-get install ca-certificates -y

USER 1000

COPY $BINARY /usr/bin/lnurl-server

ENTRYPOINT ["/usr/bin/lnurl-server"]