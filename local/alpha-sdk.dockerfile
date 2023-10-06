ARG SDK
FROM $SDK
COPY build/rpms/ /twoliter/alpha/build/rpms/
COPY sbkeys/generate-local-sbkeys /twoliter/alpha/sbkeys/generate-local-sbkeys
COPY sbkeys/generate-aws-sbkeys /twoliter/alpha/sbkeys/generate-aws-sbkeys
COPY sources/logdog/conf/current /twoliter/alpha/sources/logdog/conf/current
COPY sources/models/src/variant /twoliter/alpha/sources/models/src/variant
COPY LICENSE-APACHE /twoliter/alpha/licenses/LICENSE-APACHE
COPY LICENSE-MIT /twoliter/alpha/licenses/LICENSE-MIT
COPY COPYRIGHT /twoliter/alpha/licenses/COPYRIGHT