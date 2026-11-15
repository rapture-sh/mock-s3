# The ceph/s3-tests suite, which is the closest thing to an independent S3 compatibility
# measure. MinIO, Garage and SeaweedFS all grade themselves against it.
#
# Built here rather than pulled, because the project publishes no image.

FROM python:3.12-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /opt

# Pinned to a commit so the pass/fail list in known-failures.txt stays meaningful. An
# unpinned clone would change the goalposts between runs.
ARG S3_TESTS_REF=master
RUN git clone https://github.com/ceph/s3-tests.git \
    && cd s3-tests \
    && git checkout "$S3_TESTS_REF"

WORKDIR /opt/s3-tests
RUN pip install --no-cache-dir -r requirements.txt \
    && pip install --no-cache-dir -e .

COPY s3tests.conf /opt/s3tests.conf
ENV S3TEST_CONF=/opt/s3tests.conf

ENTRYPOINT ["python", "-m", "pytest"]
