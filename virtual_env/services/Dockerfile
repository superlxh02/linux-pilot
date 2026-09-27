FROM python:3.12-slim
WORKDIR /app
COPY demo_service.py /app/demo_service.py
RUN useradd --system --uid 10001 pilot && mkdir /data && chown pilot:pilot /data
USER pilot
ENV PYTHONUNBUFFERED=1
EXPOSE 8080
CMD ["python", "/app/demo_service.py"]
