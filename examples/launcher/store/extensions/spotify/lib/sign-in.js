// The page every command shows before the user has signed in: how to
// register a Spotify app, and a Sign In action that runs the OAuth flow and
// then lets the command render its real page.
import { Action, ActionPanel, Detail, MetadataLabel, MetadataLink, MetadataSeparator } from "launcher";
import { show_toast } from "launcher/api";
import { describe, redirectUri, signIn } from "./spotify.js";

const DASHBOARD = "https://developer.spotify.com/dashboard";

function setupMarkdown(reason) {
  return [
    "# Sign In to Spotify",
    "",
    reason ? `> ${reason}\n` : "",
    "Spotify asks every app that controls playback to be registered, so you use your own:",
    "",
    `1. Open the [Spotify Developer Dashboard](${DASHBOARD}) and choose **Create app**.`,
    "2. Tick **Web API**, and add this **Redirect URI** exactly:",
    "",
    `   \`${redirectUri()}\``,
    "",
    "3. Copy the app's **Client ID** into this extension's preferences (no client secret is needed).",
    "4. Choose **Sign In** below and approve the access in your browser.",
    "",
    "Changing the port in the preferences changes the redirect URI; register the new one too.",
    "Playback control needs Spotify Premium.",
  ].join("\n");
}

/**
 * The setup page. `on_signed_in(cx)` runs after a successful sign-in, with a
 * context that can notify the view; `reason` explains why it shows, if not
 * simply because the user has never signed in.
 */
export function signInPage(on_signed_in, reason = "") {
  const start = (cx) => {
    show_toast({ title: "Signing in…", message: "Approve the access in your browser", style: "progress", id: "sign-in" });
    cx.spawn(async (task) => {
      try {
        await signIn();
        show_toast({ title: "Signed in to Spotify", style: "success", id: "sign-in" });
        on_signed_in(task);
      } catch (error) {
        show_toast({ title: "Cannot sign in", message: describe(error), style: "failure", id: "sign-in" });
      }
    });
  };
  return new Detail(setupMarkdown(reason))
    .children([
      new MetadataLabel("Redirect URI", redirectUri()),
      new MetadataLabel("Permissions", "Playback state and control, Liked Songs"),
      new MetadataSeparator(),
      new MetadataLink("Register an App", "developer.spotify.com", DASHBOARD),
    ])
    .actions(
      new ActionPanel().children([
        new Action("Sign In").icon("log-in").run(start),
        new Action("Open Spotify Developer Dashboard").icon("external-link").open_url(DASHBOARD),
        new Action("Copy Redirect URI").icon("copy").shortcut("secondary-shift-c").copy(redirectUri()),
      ]),
    );
}
