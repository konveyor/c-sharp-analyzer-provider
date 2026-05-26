# Quick Test Commands

```bash
cd CSharpProvider

# Build
dotnet build

# Start server
dotnet run -- --port 9876 &

# List services
grpcurl -plaintext localhost:9876 list

# Init with nerd-dinner
grpcurl -plaintext -d '{"location": "/path/to/testdata/nerd-dinner/mvc4", "analysisMode": "source-only"}' localhost:9876 provider.ProviderService/Init

# Evaluate
grpcurl -plaintext -d '{"cap": "referenced", "conditionInfo": "{\"referenced\": {\"pattern\": \"System\\\\.Web.*\"}}", "id": 5}' localhost:9876 provider.ProviderService/Evaluate

# Kill server
kill $(lsof -ti:9876)
```
