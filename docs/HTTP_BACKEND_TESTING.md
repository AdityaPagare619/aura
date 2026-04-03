# HTTP Backend Testing Guide

## Quick Test (Bash Script)

Run the automated test script:

```bash
cd /data/local/tmp/aura
./test-http-backend.sh
```

Expected output:
- Checks if llama-server is running on port 8080
- Sends a test prompt via /v1/chat/completions
- Verifies we get a real AI response
- Reports pass/fail

## Manual Test (curl)

If you want to test manually:

```bash
# 1. Start llama-server (if not running)
cd /data/local/tmp/llama
./llama-server --model /data/local/tmp/aura/models/model.gguf --port 8080 --host 0.0.0.0 -c 2048 -ngl 0 &

# 2. Test health endpoint
curl http://localhost:8080/v1/models

# 3. Test chat completion
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "tinyllama",
    "messages": [{"role": "user", "content": "Hello, how are you?"}],
    "max_tokens": 50,
    "temperature": 0.7
  }'
```

## Rust Integration Test

Run the Rust integration tests:

```bash
cd /data/local/tmp/aura/aura-hotfix-link2
cargo test --test integration_http_backend -- --include-ignored
```

Note: These tests require llama-server to be running on localhost:8080.

## Troubleshooting

### "llama-server is NOT running"
Start llama-server first:
```bash
./run-llama.sh
```

### "Connection refused"
Check if port 8080 is available:
```bash
netstat -tlnp | grep 8080
```

### "Model not found"
Make sure the model file exists:
```bash
ls -la /data/local/tmp/aura/models/
```

### Empty response
Check llama-server logs for errors.
