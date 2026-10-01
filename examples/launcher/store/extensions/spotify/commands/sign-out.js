// Forgets the Spotify tokens kept in the system keychain.
import { View } from "gpui-kit";
import { List } from "launcher";
import { show_hud } from "launcher/api";
import { isSignedIn, signOut } from "../lib/spotify.js";

export default class SignOut extends View {
  init() {
    if (!isSignedIn()) {
      show_hud("Not signed in to Spotify");
      return;
    }
    signOut();
    show_hud("Signed out of Spotify");
  }

  render() {
    return new List();
  }
}
