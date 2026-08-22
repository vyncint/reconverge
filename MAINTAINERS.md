# Maintainers

| Maintainer | GitHub | Areas |
| --- | --- | --- |
| Vyncint Ng | [@vyncint](https://github.com/vyncint) | everything |

reconverge is solo-maintained, which is worth knowing before you open a pull
request: review is usually within a few days, and a small focused change is
reviewed faster than a large one.

## What a maintainer does here

- **Reviews and merges.** PRs are squash-merged; the PR title becomes the
  commit subject on `main`.
- **Cuts releases**, following [docs/RELEASING.md](docs/RELEASING.md).
  Publishing is by dispatch with a dry run first, because it is irreversible.
- **Owns the pins.** The nightly toolchain moves only in lockstep with the
  upstream cuda-oxide pin, never together with a change to analysis behaviour.

## Becoming one

Land a few changes that need no rework, review someone else's, and say you are
interested. There is no committee.

## Security

Do not open a public issue for a vulnerability — see
[SECURITY.md](SECURITY.md).
