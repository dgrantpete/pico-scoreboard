FROM python:3.13-slim

ENV PYTHONUNBUFFERED=1
WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# GIT_SHA below the pip layer so a new commit never busts the dependency cache.
ARG GIT_SHA=unknown
ENV COLLECTOR_VERSION=$GIT_SHA
COPY . espn/

CMD ["python", "-m", "espn", "serve", "--targets", "/config/targets.yml"]
