ARG SDK
ARG HOST_GOARCH

FROM $SDK
FROM --platform=linux/${HOST_GOARCH} $SDK

COPY build/rpms/*.rpm /twoliter/alpha/build/rpms/
COPY build/rpms/*/*.rpm /twoliter/alpha/build/rpms/

# These may need to be moved to Twoliter, but for now we will access them from the Alpha SDK.
# They have been moved into the .cargo directory because they are otherwise .dockerignored.
COPY .cargo/sbkeys/generate-local-sbkeys /twoliter/alpha/sbkeys/generate-local-sbkeys
COPY .cargo/sbkeys/generate-aws-sbkeys /twoliter/alpha/sbkeys/generate-aws-sbkeys

# TODO - move these to an RPM package so we don't need to copy them here.
COPY LICENSE-APACHE /twoliter/alpha/licenses/LICENSE-APACHE
COPY LICENSE-MIT /twoliter/alpha/licenses/LICENSE-MIT
COPY COPYRIGHT /twoliter/alpha/licenses/COPYRIGHT
