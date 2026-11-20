# Mech Royale

Oval Runner is a native Rust bot for the Builders mech competition. The root
Dockerfile builds the locked Rust project and packages only the executable in
the runtime image.

[Official track brief and submission page](https://www.joinbuilders.org/buildathon/tracks/builders)

The submission cutoff is Sunday 22 November 2026 at 11 am ET (4 pm UK time).

## Local checks

Run from the repository root:

```sh
cargo test --locked
docker build --platform linux/amd64 -t oval-runner .
```

Keep the official kit in the ignored `.buildathon/` directory. From the
extracted kit directory, run the official conformance check:

```sh
python3 check/check.py --map-dir /arena -- docker run --rm -i --network none --cpus 1 --memory 512m --read-only --tmpfs /tmp:rw,size=64m --user 65534:65534 -v "$PWD/arena:/arena:ro" oval-runner
```

The checker replays recorded observations and validates the protocol. Practice
placement is measured on the official track. The earlier Python bot is retained
for comparison.
