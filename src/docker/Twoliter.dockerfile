# syntax=docker/dockerfile:1.4.3
ARG BASE
FROM ${BASE} as base

COPY --chmod=755 buildsys /usr/local/bin
