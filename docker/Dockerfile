FROM alpine:3.12.0

COPY ./ebbflow /
COPY ./entrypoint.sh /

ENTRYPOINT [ "/entrypoint.sh" ]
CMD ["run-blocking", "--help"]
