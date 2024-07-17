.PHONY: design

design: ## render design diagrams
	./docs/design/bin/render-plantuml.sh \
		./docs/design/diagrams/build-sequence.plantuml \
		./docs/design/diagrams/build-sequence.svg

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
check: fmt clippy deny test integ

.PHONY: build
build: check
	cargo build --release --locked
