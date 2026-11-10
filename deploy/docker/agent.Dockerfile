FROM registry.k8s.io/kubectl:v1.36.0 AS kubectl
FROM python:3.13.12-slim-bookworm
WORKDIR /app
COPY LICENSE COMMERCIAL-LICENSING.md NOTICE /usr/share/licenses/spitzli-cloud/
COPY --from=kubectl /bin/kubectl /usr/local/bin/kubectl
COPY agents/kubernetes/*.py ./
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 SPITZLI_KUBECTL_CACHE_DIR=/tmp/kube-cache
USER 65532:65532
ENTRYPOINT ["python3", "daemon.py", "--byok", "--credentials", "/state/connection.json"]
