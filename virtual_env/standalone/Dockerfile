FROM python:3.12-slim-bookworm

# 每个容器同时运行一个单体应用和一个独立 Worker。它们共享该容器的
# PID/cgroup 视图，故此镜像只模拟三台机器的部署边界，并非三个真实内核。
RUN apt-get update && apt-get install -y --no-install-recommends linux-perf ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=linux-pilot-worker:local /usr/local/bin/linux-pilot-worker /usr/local/bin/linux-pilot-worker
COPY service.py /opt/linux-pilot/service.py
COPY start.sh /usr/local/bin/pilot-standalone-start
RUN chmod +x /usr/local/bin/pilot-standalone-start
ENV PO_AGENT__SPOOL_DIR=/var/lib/linux-pilot-worker/spool
EXPOSE 8080
CMD ["/usr/local/bin/pilot-standalone-start"]
