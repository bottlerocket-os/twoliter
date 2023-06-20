.PHONY: design

design: ## render design diagrams
	./docs/design/bin/render-plantuml.sh \
		./docs/design/diagrams/build-sequence.plantuml \
		./docs/design/diagrams/build-sequence.svg
