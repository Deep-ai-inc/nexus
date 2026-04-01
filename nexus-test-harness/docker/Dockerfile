FROM alpine:3.19

RUN apk add --no-cache \
        openssh-server \
        iproute2 \
        iptables \
        procps \
        bash \
    && ssh-keygen -A

# Test user with key auth
RUN adduser -D -s /bin/bash testuser \
    && passwd -u testuser \
    && mkdir -p /home/testuser/.ssh /home/testuser/.nexus \
    && chown -R testuser:testuser /home/testuser

COPY id_test.pub /home/testuser/.ssh/authorized_keys
RUN chown -R testuser:testuser /home/testuser \
    && chmod 700 /home/testuser/.ssh \
    && chmod 600 /home/testuser/.ssh/authorized_keys \
    && chmod 755 /home/testuser

# Aggressive sshd keepalives for fast dead-connection detection
RUN echo "ClientAliveInterval 2" >> /etc/ssh/sshd_config \
    && echo "ClientAliveCountMax 2" >> /etc/ssh/sshd_config

EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
