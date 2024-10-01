TOP := $(dir $(abspath $(firstword $(MAKEFILE_LIST))))

BOTTLEROCKET_SDK_VERSION ?= v0.45.0
BOTTLEROCKET_SDK_IMAGE ?= public.ecr.aws/bottlerocket/bottlerocket-sdk:$(BOTTLEROCKET_SDK_VERSION)

.PHONY: design
design: ## render design diagrams
	./docs/design/bin/render-plantuml.sh \
		./docs/design/diagrams/build-sequence.plantuml \
		./docs/design/diagrams/build-sequence.svg

.PHONY: attributions
attributions:
	docker build \
		--build-arg BOTTLEROCKET_SDK_IMAGE=$(BOTTLEROCKET_SDK_IMAGE) \
		--build-arg UID=$(shell id -u) \
		--build-arg GID=$(shell id -g) \
		--tag twoliter-attributions-image:latest \
		-f "$(TOP)/tools/attribution/Dockerfile.attribution" \
		.
	docker run --rm \
		--volume "$(TOP):/src" \
		--user "$(shell id -u):$(shell id -g)" \
		--security-opt label=disable \
		--workdir "/src" \
		twoliter-attributions-image:latest \
		bash -c "/src/tools/attribution/attribution.sh"

	docker rmi twoliter-attributions-image:latest

.PHONY: deny
deny:
	cargo deny --no-default-features check licenses bans sources

.PHONY: clippy
clippy:
	cargo clippy --locked -- -D warnings --no-deps

.PHONY: fmt
fmt:
	cargo fmt --check

.PHONY: test
test:
	cargo test --release --locked

.PHONY: integ
integ:
	cargo test --manifest-path tests/integration-tests/Cargo.toml -- --include-ignored

.PHONY: check
check: fmt clippy deny attributions test integ

.PHONY: build
build: check
	cargo build --release --locked
