download_proto:
	curl -L -o src/build/proto/provider.proto https://raw.githubusercontent.com/konveyor/analyzer-lsp/refs/heads/main/provider/internal/grpc/library.proto

build_grpc:
	cargo build

run:
	cargo run  -- --port 9000 --name c-sharp --db-path testing.db

build-image:
	docker build -f dotnet-base-provider.Dockerfile .

run-grpc-init-http:
	grpcurl -max-time 1000 -plaintext -d '{"analysisMode": "source-only", "location": "$(PWD)/testdata/nerd-dinner", "providerSpecificConfig": {"ilspy_cmd": "${HOME}/.dotnet/tools/ilspycmd", "paket_cmd": "${HOME}/.dotnet/tools/paket"}}' localhost:9000 provider.ProviderService.Init

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

make-demo-output-local:
	sed -i.bak "s#<REPLACE_ME>#$(PWD)#g" demo-output.yaml

run-demo: reset-demo-apps build_grpc make-demo-output-local
	export SERVER_PID=$$(./scripts/run-demo.sh); \
	echo $${SERVER_PID}; \
	$(MAKE) wait-for-server; \
	$(MAKE) run-grpc-init-http; \
	$(MAKE) run-grpc-ref-http; \
	kill $${SERVER_PID}; \
	diff output.yaml demo-output.yaml
	$(MAKE) reset-demo-apps

run-demo-github: reset-demo-apps build_grpc make-demo-output-local
	RUST_LOG=c_sharp_analyzer_provider_cli=DEBUG,INFO target/debug/c-sharp-analyzer-provider-cli --port 9000 --name c-sharp &> demo.log
	$(MAKE) wait-for-server;
	$(MAKE) run-grpc-init-http;
	$(MAKE) run-grpc-ref-http;
	cat output.yaml
	diff demo-output.yaml output.yaml
	$(MAKE) reset-demo-apps;

