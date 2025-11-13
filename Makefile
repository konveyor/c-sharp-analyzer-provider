# Container runtime (podman by default, can be overridden with docker)
CONTAINER_RUNTIME ?= podman

# Branch to download konveyor-analyzer from (defaults to main)
KONVEYOR_BRANCH ?= main

.PHONY: download_proto build_grpc run build-image run-grpc-init-http run-grpc-ref-http wait-for-server reset-nerd-dinner-demo reset-demo-apps reset-demo-output run-demo run-demo-github run-integration-tests container-build-test container-run-test container-test get-konveyor-analyzer update-provider-settings run-tests verify-output

download_proto:
	curl -L -o src/build/proto/provider.proto https://raw.githubusercontent.com/konveyor/analyzer-lsp/refs/heads/main/provider/internal/grpc/library.proto

build_grpc:
	cargo build

run:
	cargo run  -- --port 9000 --name c-sharp --db-path testing.db

build-image:
	$(CONTAINER_RUNTIME) build -f dotnet-base-provider.Dockerfile -t quay.io/kovneyor/c-sharp-external-provider .

run-grpc-init-http:
	grpcurl -max-time 1000 -plaintext -d "{\"analysisMode\": \"source-only\", \"location\": \"$(PWD)/testdata/nerd-dinner\", \"providerSpecificConfig\": {\"ilspy_cmd\": \"$$HOME/.dotnet/tools/ilspycmd\", \"paket_cmd\": \"$$HOME/.dotnet/tools/paket\"}}" localhost:9000 provider.ProviderService.Init

run-grpc-ref-http:
	grpcurl -max-msg-sz 10485760 -max-time 30 -plaintext -d '{"cap": "referenced", "conditionInfo": "{\"referenced\": {\"pattern\": \"System.Web.Mvc.*\"}}" }' -connect-timeout 5000.000000 localhost:9000 provider.ProviderService.Evaluate > output.yaml

wait-for-server:
	@echo "Waiting for server to start listening on localhost:9000..."
	@for i in $(shell seq 1 300); do \
		if nc -z localhost 9000; then \
			echo "Server is listening!"; \
			exit 0; \
		else \
			echo "Attempt $$i: Server not ready. Waiting 1s..."; \
			sleep 1; \
		fi; \
	done

reset-nerd-dinner-demo:
	cd testdata/nerd-dinner && rm -rf paket-files && rm -rf packages && git clean -f . && git stash push .

reset-demo-apps: reset-nerd-dinner-demo reset-demo-output
	rm -f demo.db

reset-demo-output:
	@if [ -f "demo-output.yaml.bak" ]; then \
		mv demo-output.yaml.bak demo-output.yaml; \
	fi

run-demo: reset-demo-apps build_grpc
	export SERVER_PID=$$(./scripts/run-demo.sh); \
	echo $${SERVER_PID}; \
	$(MAKE) wait-for-server; \
	$(MAKE) run-grpc-init-http; \
	$(MAKE) run-integration-tests; \
	kill $${SERVER_PID}; \
	$(MAKE) reset-demo-apps

run-demo-github: reset-demo-apps build_grpc
	RUST_LOG=c_sharp_analyzer_provider_cli=DEBUG,INFO target/debug/c-sharp-analyzer-provider-cli --port 9000 --name c-sharp &> demo.log & \
	export SERVER_PID=$$!; \
	$(MAKE) wait-for-server; \
	$(MAKE) run-grpc-init-http; \
	$(MAKE) run-integration-tests; \
	kill $$SERVER_PID || true; \
	$(MAKE) reset-demo-apps

run-demo-container: build_grpc
	RUST_LOG=c_sharp_analyzer_provider_cli=DEBUG,INFO target/debug/c-sharp-analyzer-provider-cli --port 9000 --name c-sharp &> demo.log & \
	export SERVER_PID=$$!; \
	$(MAKE) wait-for-server; \
	$(MAKE) run-grpc-init-http; \
	$(MAKE) run-integration-tests; \

run-integration-tests:
	cargo test -- --nocapture

# Container-based integration testing (uses CONTAINER_RUNTIME variable)
container-build-test:
	$(CONTAINER_RUNTIME) build -f Dockerfile.test -t c-sharp-provider-test:latest .

container-run-test:
	$(CONTAINER_RUNTIME) run --rm c-sharp-provider-test:latest

container-test: container-build-test container-run-test

get-konveyor-analyzer:
	@if [ -f "e2e-tests/konveyor-analyzer" ]; then \
		echo "konveyor-analyzer already exists in e2e-tests/"; \
	elif command -v konveyor-analyzer >/dev/null 2>&1; then \
		echo "konveyor-analyzer found in PATH, copying to e2e-tests/"; \
		cp $$(command -v konveyor-analyzer) e2e-tests/konveyor-analyzer; \
	else \
		echo "konveyor-analyzer not found. Downloading from GitHub (branch: $(KONVEYOR_BRANCH))..."; \
		if ! command -v gh >/dev/null 2>&1; then \
			echo "Error: 'gh' CLI is required to download artifacts. Please install it from https://cli.github.com/"; \
			exit 1; \
		fi; \
		mkdir -p e2e-tests; \
		OS=$$(uname -s | tr '[:upper:]' '[:lower:]'); \
		ARCH=$$(uname -m); \
		if [ "$$ARCH" = "x86_64" ]; then ARCH="amd64"; elif [ "$$ARCH" = "aarch64" ]; then ARCH="arm64"; fi; \
		PLATFORM="$$OS-$$ARCH"; \
		echo "Detected platform: $$PLATFORM"; \
		cd e2e-tests && \
		echo "Fetching latest successful workflow run from $(KONVEYOR_BRANCH) branch..." && \
		RUN_ID=$$(gh run list --repo konveyor/analyzer-lsp --branch $(KONVEYOR_BRANCH) --status success --workflow "Build and Test" --limit 1 --json databaseId --jq '.[0].databaseId'); \
		if [ -z "$$RUN_ID" ] || [ "$$RUN_ID" = "null" ]; then \
			echo "Error: No successful workflow runs found on branch $(KONVEYOR_BRANCH)"; \
			exit 1; \
		fi; \
		echo "Latest successful run ID: $$RUN_ID"; \
		gh run download $$RUN_ID --repo konveyor/analyzer-lsp --dir . && \
		echo "Downloaded artifacts. Extracting binaries for platform $$PLATFORM..." && \
		ARTIFACT_DIR=$$(find . -type d -name "*$$PLATFORM*" | head -1); \
		if [ -z "$$ARTIFACT_DIR" ]; then \
			echo "Error: No artifact found for platform $$PLATFORM. Available artifacts:"; \
			ls -la; \
			exit 1; \
		fi; \
		echo "Found artifact directory: $$ARTIFACT_DIR"; \
		unzip -q "$$ARTIFACT_DIR"/*.zip -d extracted && \
		if [ -f "extracted/konveyor-analyzer" ]; then \
			mv extracted/konveyor-analyzer konveyor-analyzer; \
			chmod +x konveyor-analyzer; \
			rm -rf extracted analyzer-lsp-binaries.*; \
			echo "Successfully downloaded konveyor-analyzer to e2e-tests/"; \
		else \
			echo "Error: konveyor-analyzer binary not found in extracted files:"; \
			find extracted -type f; \
			exit 1; \
		fi; \
	fi

update-provider-settings:
	@echo "Updating provider_settings.json with current paths..."
	@if ! command -v jq >/dev/null 2>&1; then \
		echo "Error: 'jq' is required to update provider settings. Please install it."; \
		exit 1; \
	fi
	@CURRENT_DIR=$$(pwd); \
	BINARY_PATH="$$CURRENT_DIR/target/debug/c-sharp-analyzer-provider-cli"; \
	LOCATION_PATH="$$CURRENT_DIR/testdata/nerd-dinner"; \
	ILSPY_CMD="$$HOME/.dotnet/tools/ilspycmd"; \
	PAKET_CMD="$$HOME/.dotnet/tools/paket"; \
	jq --arg bp "$$BINARY_PATH" \
	   --arg loc "$$LOCATION_PATH" \
	   --arg ilspy "$$ILSPY_CMD" \
	   --arg paket "$$PAKET_CMD" \
	   '.[0].binaryPath = $$bp | .[0].initConfig[0].location = $$loc | .[0].initConfig[0].providerSpecificConfig.ilspy_cmd = $$ilspy | .[0].initConfig[0].providerSpecificConfig.paket_cmd = $$paket' \
	   e2e-tests/provider_settings.json > e2e-tests/provider_settings.json.tmp && \
	mv e2e-tests/provider_settings.json.tmp e2e-tests/provider_settings.json
	@echo "Updated provider_settings.json"

run-tests: update-provider-settings
	@echo "Running konveyor-analyzer with rulesets..."
	@ANALYZER_BIN=""; \
	if [ -f "e2e-tests/konveyor-analyzer" ]; then \
		ANALYZER_BIN="./e2e-tests/konveyor-analyzer"; \
	elif command -v konveyor-analyzer >/dev/null 2>&1; then \
		ANALYZER_BIN="konveyor-analyzer"; \
	else \
		echo "Error: konveyor-analyzer not found. Run 'make get-konveyor-analyzer' first."; \
		exit 1; \
	fi; \
	echo "Using analyzer: $$ANALYZER_BIN"; \
	$$ANALYZER_BIN \
		--provider-settings e2e-tests/provider_settings.json \
		--rules rulesets/ \
		--output-file e2e-tests/analysis-output.yaml

verify-output:
	@echo "Verifying analysis output matches expected demo output..."
	@if [ ! -f "e2e-tests/analysis-output.yaml" ]; then \
		echo "Error: analysis-output.yaml not found. Run 'make run-tests' first."; \
		exit 1; \
	fi
	@if [ ! -f "e2e-tests/demo-output.yaml" ]; then \
		echo "Error: demo-output.yaml not found."; \
		exit 1; \
	fi
	@if diff -u e2e-tests/demo-output.yaml e2e-tests/analysis-output.yaml > /dev/null 2>&1; then \
		echo "✓ Output matches! Analysis results are correct."; \
	else \
		echo "✗ Output differs from expected results:"; \
		diff -u e2e-tests/demo-output.yaml e2e-tests/analysis-output.yaml || true; \
		exit 1; \
	fi

run-e2e-demo: get-konveyor-analyzer run-tests verify-output
