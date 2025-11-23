# Container runtime (podman by default, can be overridden with docker)
CONTAINER_RUNTIME ?= podman

# Branch to download konveyor-analyzer from (defaults to main)
KONVEYOR_BRANCH ?= main

# SELinux label for shared volumes (use :z for shared, :Z for exclusive)
MOUNT_OPT ?= :z

TAG ?= latest
IMAGE ?= c-sharp-provider:${TAG}
IMG_ANALYZER ?= quay.io/konveyor/analyzer-lsp:$(TAG)

.PHONY: all clean test download_proto build run build-image run-grpc-init-http run-grpc-ref-http wait-for-server reset-nerd-dinner-demo reset-demo-apps reset-demo-output run-tests run-tests-manual run-integration-tests get-konveyor-analyzer-local update-provider-settings-local run-test-local verify-output verify-e2e-results run-analyzer-integration-local run-c-sharp-pod stop-c-sharp-pod run-demo-c-sharp-pod run-analyzer-integration

all: build

clean: reset-demo-apps
	cargo clean
	rm -f e2e-tests/konveyor-analyzer
	rm -f e2e-tests/analysis-output.yaml

test: run-tests

download_proto:
	curl -L -o src/build/proto/provider.proto https://raw.githubusercontent.com/konveyor/analyzer-lsp/refs/heads/main/provider/internal/grpc/library.proto

build:
	cargo build

run:
	cargo run  -- --port 9000 --name c-sharp --db-path testing.db

build-image:
	$(CONTAINER_RUNTIME) build -f Dockerfile -t ${IMAGE} .

### Local GRPC testing
run-grpc-init-http:
	grpcurl -max-time 1000 -plaintext -d "{\"analysisMode\": \"source-only\", \"location\": \"$(PWD)/testdata/nerd-dinner\", \"providerSpecificConfig\": {\"ilspy_cmd\": \"$$HOME/.dotnet/tools/ilspycmd\", \"paket_cmd\": \"$$HOME/.dotnet/tools/paket\", \"dotnet_install_cmd\": \"$(PWD)/scripts/dotnet-install.sh\"}}" localhost:9000 provider.ProviderService.Init

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

reset-net8-sample:
	cd testdata/net8-sample && rm -rf paket* && rm -rf .paket && rm -rf obj && git clean -f . && git stash push .

reset-demo-apps: reset-nerd-dinner-demo reset-net8-sample reset-demo-output
	rm -f demo.db test*.db test*.log


reset-demo-output:
	@if [ -f "demo-output.yaml.bak" ]; then \
		mv demo-output.yaml.bak demo-output.yaml; \
	fi

# Integration tests now manage server lifecycle automatically
run-tests: reset-demo-apps build
	cargo test -- --nocapture; \
	TEST_EXIT=$$?; \
	$(MAKE) reset-demo-apps; \
	exit $$TEST_EXIT

run-integration-tests:
	cargo test -- --nocapture

# Legacy target for manual server management (deprecated)
run-tests-manual: reset-demo-apps build
	export SERVER_PID=$$(./scripts/run-demo.sh); \
	echo $${SERVER_PID}; \
	$(MAKE) wait-for-server && \
	$(MAKE) run-grpc-init-http && \
	$(MAKE) run-integration-tests; \
	TEST_EXIT=$$?; \
	kill $${SERVER_PID} || true; \
	$(MAKE) reset-demo-apps; \
	exit $$TEST_EXIT


## Running analyzer integration test locally.
get-konveyor-analyzer-local:
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

update-provider-settings-local:
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

run-test-local: update-provider-settings-local
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

verify-e2e-results:
	@echo "Verifying e2e results with sorted incidents..."
	@if [ ! -f "e2e-tests/demo-output.yaml" ]; then \
		echo "Error: demo-output.yaml not found."; \
		exit 1; \
	fi
	@if ! command -v yq >/dev/null 2>&1; then \
		echo "Error: 'yq' is required to sort YAML. Please install it from https://github.com/mikefarah/yq"; \
		exit 1; \
	fi
	@echo "Sorting incidents in both files..."
	@git show HEAD:e2e-tests/demo-output.yaml | yq eval '.[] | .violations |= (map(.incidents |= sort_by(.uri, .lineNumber, .message)))' > /tmp/demo-head-sorted.yaml
	@yq eval '.[] | .violations |= (map(.incidents |= sort_by(.uri, .lineNumber, .message)))' e2e-tests/demo-output.yaml > /tmp/demo-current-sorted.yaml
	@if diff -u /tmp/demo-head-sorted.yaml /tmp/demo-current-sorted.yaml > /dev/null 2>&1; then \
		echo "✓ Output matches (after sorting)! Changes are only ordering differences."; \
		rm -f /tmp/demo-head-sorted.yaml /tmp/demo-current-sorted.yaml; \
	else \
		echo "✗ Output differs from HEAD (even after sorting):"; \
		diff -u /tmp/demo-head-sorted.yaml /tmp/demo-current-sorted.yaml || true; \
		echo ""; \
		echo "Sorted files saved to /tmp/demo-head-sorted.yaml and /tmp/demo-current-sorted.yaml for inspection"; \
		exit 1; \
	fi

run-analyzer-integration-local: get-konveyor-analyzer-local run-test-local verify-output 

## Running analyzer integration test as you would in CI.
run-c-sharp-pod:
	$(CONTAINER_RUNTIME) volume create test-data
	$(CONTAINER_RUNTIME) run --rm -v test-data:/target$(MOUNT_OPT) -v $(PWD)/testdata:/src/$(MOUNT_OPT) --entrypoint=cp alpine -a /src/. /target/
	$(CONTAINER_RUNTIME) pod create --name=analyzer-c-sharp
	$(CONTAINER_RUNTIME) run --pod analyzer-c-sharp --name c-sharp -d -v test-data:/analyzer-lsp/examples$(MOUNT_OPT) ${IMAGE} --port 14652

stop-c-sharp-pod:
	$(CONTAINER_RUNTIME) pod kill analyzer-c-sharp || true
	$(CONTAINER_RUNTIME) pod rm analyzer-c-sharp || true
	$(CONTAINER_RUNTIME) volume rm test-data || true

run-demo-c-sharp-pod:
	$(CONTAINER_RUNTIME) run --entrypoint /usr/local/bin/konveyor-analyzer --pod=analyzer-c-sharp\
		-v test-data:/analyzer-lsp/examples$(MOUNT_OPT) \
		-v $(PWD)/e2e-tests/demo-output.yaml:/analyzer-lsp/output.yaml$(MOUNT_OPT) \
		-v $(PWD)/e2e-tests/provider_settings.json:/analyzer-lsp/provider_settings.json$(MOUNT_OPT) \
		-v $(PWD)/rulesets/:/analyzer-lsp/rules$(MOUNT_OPT) \
		$(IMG_ANALYZER) \
		--verbose=100 \
		--output-file=/analyzer-lsp/output.yaml \
		--rules=/analyzer-lsp/rules \
		--provider-settings=/analyzer-lsp/provider_settings.json

run-analyzer-integration: run-c-sharp-pod run-demo-c-sharp-pod stop-c-sharp-pod
