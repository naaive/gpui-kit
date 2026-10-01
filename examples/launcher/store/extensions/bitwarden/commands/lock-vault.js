// A no-view command: runs `bw lock`, which locks the vault and makes every
// session key unusable, then reports with a HUD.
import { View } from "gpui-kit";
import { List } from "launcher";
import { show_hud } from "launcher/api";
import { BwError, lock } from "../lib/bw.js";

export default class LockVault extends View {
  init(_props, cx) {
    cx.spawn(async () => {
      try {
        await lock();
        show_hud("Vault Locked");
      } catch (error) {
        show_hud(error instanceof BwError ? error.title : "Cannot lock the vault");
      }
    });
  }

  render() {
    return new List();
  }
}
