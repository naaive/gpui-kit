// Runs the docker CLI and reads what it prints. Every listing asks for
// `--format {{json .}}`, one JSON object per line. A missing CLI or a stopped
// daemon becomes a `DockerError` whose title and message the page shows as
// its empty state.
import { run } from "process";
import { launch } from "launcher/api";

export class DockerError extends Error {
  constructor(title, message) {
    super(message);
    this.title = title;
  }
}

function hostArguments() {
  const host = String(launch().preferences.host ?? "").trim();
  return host ? ["--host", host] : [];
}

function explain(output) {
  const text = `${output}`.trim();
  if (/not found on the host PATH|cannot find|No such file|is not recognized/i.test(text)) {
    return new DockerError("Docker Is Not Installed", "Install Docker Desktop or the docker CLI, and make sure `docker` is on PATH.");
  }
  if (/not granted/i.test(text)) {
    return new DockerError("Docker Is Not Allowed", "Allow this extension to run `docker` in its permissions.");
  }
  if (/Cannot connect to the Docker daemon|error during connect|daemon running|docker_engine|docker\.sock|The system cannot find the file specified/i.test(text)) {
    return new DockerError(
      "Docker Is Not Running",
      "Start Docker Desktop (or the Docker daemon), then refresh. To use another engine, set Docker Host in the extension's preferences.",
    );
  }
  const first = text.split("\n")[0].replace(/^Error response from daemon:\s*/i, "");
  return new DockerError("Docker Failed", first || "docker answered with an error.");
}

/** Runs `docker <args>` and answers `{ stdout, stderr }`, or throws a `DockerError`. */
export async function invoke(args) {
  let output;
  try {
    output = await run("docker", [...hostArguments(), ...args]);
  } catch (error) {
    throw explain(error?.message ?? error);
  }
  if (output.code !== 0) throw explain(output.stderr || output.stdout);
  return output;
}

/** Runs `docker <args>` and answers its standard output. */
export async function docker(args) {
  return (await invoke(args)).stdout;
}

/** One JSON object per non-empty line. */
export function parseLines(text) {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.startsWith("{"))
    .map((line) => {
      try {
        return JSON.parse(line);
      } catch {
        return null;
      }
    })
    .filter(Boolean);
}

/** The first line of an error, for a toast. */
export function describe(error) {
  if (error instanceof DockerError) return [error.title, error.message];
  return ["Docker Failed", String(error?.message ?? error)];
}
