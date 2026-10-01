# Docker

Manage local Docker containers and images with the `docker` CLI.

## Commands

- **Search Docker Containers**: running and stopped containers, with their
  state, image and published ports. Start, stop or restart one, view its last
  200 log lines (with Refresh), open a published port in the browser, copy its
  ID, or remove it (asks first; a running container is stopped and removed).
  The dropdown narrows the list to running or stopped containers.
- **Search Docker Images**: local images with size and age. Run a container
  from one (name, published ports and cleanup in a form), copy its name, ID or
  a `docker run` command, or remove it (asks first).

When `docker` is not installed or its daemon is not running, the list says so
instead of showing an error.

## Setup

Install Docker Desktop (or the Docker Engine) so that `docker` is on `PATH`.
The launcher runs `docker` without your shell's environment, so `DOCKER_HOST`
and the current `docker context` are not seen and docker talks to its default
engine. To use another one, set **Docker Host** in the extension's
preferences (for example `unix:///var/run/docker.sock` or
`npipe:////./pipe/dockerDesktopLinuxEngine`).

## Permissions

- Run the `docker` program (`fs.execute: ["docker"]`). Nothing else: no files
  and no network.
