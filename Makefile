.PHONY: help build build-api build-lb test config clean up down

help:
	@echo "Available targets:"
	@echo "  build     - Build local API and LB Docker images"
	@echo "  build-api - Build the local API Docker image (rinha-rust-tree-api:local)"
	@echo "  build-lb  - Build the local LB Docker image (rinha-rust-tree-lb:local)"
	@echo "  test    - Run Rust unit tests"
	@echo "  config  - Validate the local Docker Compose syntax"
	@echo "  up      - Start the local Docker Compose stack"
	@echo "  down    - Stop the local Docker Compose stack"
	@echo "  clean   - Clean cargo build artifacts"

build: build-api build-lb

build-api:
	@docker build -f docker/Dockerfile --target api-runtime -t rinha-rust-tree-api:local .

build-lb:
	@docker build -f docker/Dockerfile --target lb-runtime -t rinha-rust-tree-lb:local .

test:
	@cargo test

config:
	@docker compose -f docker/compose.local.yml config -q

up:
	@docker compose -f docker/compose.local.yml up -d

down:
	@docker compose -f docker/compose.local.yml down -v --remove-orphans

clean:
	@cargo clean
