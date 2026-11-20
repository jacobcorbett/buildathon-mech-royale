# Mech Royale

Buildathon entry for the Builders mech-bot competition.

[Official track brief and submission page](https://www.joinbuilders.org/buildathon/tracks/builders)

The submission cutoff is Sunday 22 November 2026 at 11 am ET (4 pm UK time).

## Local setup

The official starter archive and protocol are in `.buildathon/` on Yoga. Extract the archive into that directory to inspect the Python and TypeScript bots and the conformance check. Docker is available on Yoga.

The scorer builds the submitted commit's top-level Dockerfile for linux/amd64. The bot exchanges JSON lines over stdin/stdout, runs offline with one CPU and 512 MB RAM, and has 50 ms per observation. Practice ranks mean placement points. The finalists play each other.
