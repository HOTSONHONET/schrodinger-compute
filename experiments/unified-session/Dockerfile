FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --create-home compute \
    && install -d -o compute -g compute /data
WORKDIR /opt/schrodinger
COPY protocol.py worker.py ./
USER compute
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
EXPOSE 7353
CMD ["python3", "worker.py"]
