# DevOps, Infrastructure, and CI Architecture (Code-First)

## 1) CI pipeline topology

Main CI workflow:

- `/home/runner/work/aura/aura/.github/workflows/ci.yml` (named `CI Pipeline v2`)

Key gates:

- `cargo check --workspace --features "aura-llama-sys/stub,aura-daemon/voice,aura-daemon/reqwest"`
- `cargo test --workspace --features "aura-llama-sys/stub,aura-daemon/voice,aura-daemon/reqwest"`
- `cargo clippy --workspace --features "aura-llama-sys/stub,aura-daemon/voice,aura-daemon/reqwest" -- -D warnings`
- `cargo fmt --check`
- `cargo audit`

## 2) Android pipeline

Dedicated Android build workflow:

- `/home/runner/work/aura/aura/.github/workflows/build-android.yml`

Observed architecture:

- NDK pinned and checksum-verified,
- explicit linker/toolchain env wiring,
- cross-compilation artifacts uploaded,
- runtime dependency check to avoid `libc++_shared.so` reliance.

## 3) Artifact handoff and downstream validation

Artifact names and consumer workflows are explicitly coupled:

- producer artifact in CI: `aura-daemon-android-v2`
- consumer: `device-validate.yml`

References:

- `/home/runner/work/aura/aura/.github/workflows/ci.yml`
- `/home/runner/work/aura/aura/.github/workflows/device-validate.yml`

## 4) Install/release operational infra

- Installer: `/home/runner/work/aura/aura/install.sh`
- developer tasking: `/home/runner/work/aura/aura/Makefile`
- infra scripts/tests/docs: `/home/runner/work/aura/aura/infrastructure/`

## 5) Infrastructure outcome

The repository has mature CI segmentation and Android-focused reproducibility controls, with clear room for tighter workflow coupling checks to prevent artifact mismatch regressions.
