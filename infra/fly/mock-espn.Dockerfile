# Mock-ESPN for the Fly staging pair. Build context is the REPO ROOT (the
# tools/espn collector Dockerfile's context can't reach backend/testdata or
# the bundles). The staging demo config is baked in — changing the demo is an
# edit to infra/fly/mock.staging.yml (and/or re-exported bundles) + redeploy.
FROM python:3.13-slim

ENV PYTHONUNBUFFERED=1
WORKDIR /app
COPY tools/espn/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY tools/espn/ espn/
COPY backend/testdata/ testdata/
COPY data/espn/bundles/ bundles/
COPY infra/fly/mock.staging.yml mock.yml

CMD ["python", "-m", "espn", "mock", "--config", "/app/mock.yml", "--testdata", "/app/testdata", "--port", "8787"]
