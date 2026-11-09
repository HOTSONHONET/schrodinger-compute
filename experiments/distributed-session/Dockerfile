FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends python3 ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --create-home compute
WORKDIR /opt/schrodinger
COPY worker.py session.py ./
USER compute
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 SC_WORKSPACE=/workspace
EXPOSE 7351
CMD ["python3", "worker.py"]
