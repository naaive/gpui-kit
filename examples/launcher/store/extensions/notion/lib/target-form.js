// What Create Page and Append to Page share: a form whose first field picks
// a page (or database) shared with the integration, loaded from Notion's
// search, with the one chosen last chosen again.
import { View } from "gpui-kit";
import { FormDescription } from "launcher";
import { open, pop, show_toast } from "launcher/api";
import { describe, search, titleOf } from "./notion.js";

export class TargetForm extends View {
  /**
   * `target` is a page or database chosen already (pushed from Search
   * Notion); `object` limits the choices to `"page"`; `remember_key` is where
   * the last choice is kept.
   */
  init({ target = null, object = null, remember_key }, cx) {
    this.target = target;
    this.object = object;
    this.remember_key = remember_key;
    this.targets = target ? [target] : [];
    this.loading = true;
    this.saving = false;
    this.error = null;
    this.loadTargets(cx);
  }

  remembered() {
    try {
      return localStorage.getItem(this.remember_key);
    } catch {
      return null;
    }
  }

  remember(id) {
    try {
      localStorage.setItem(this.remember_key, id);
    } catch {
      // Only a convenience.
    }
  }

  /** The id the dropdown starts on; a subclass's FormState takes it. */
  chooseInitial(id) {}

  loadTargets(cx) {
    cx.spawn(async (task) => {
      try {
        const found = await search("", { object: this.object, page_size: 50 });
        const extra = found.filter((each) => each.id !== this.target?.id);
        this.targets = this.target ? [this.target, ...extra] : extra;
        const remembered = this.targets.find((each) => each.id === this.remembered());
        const initial = this.target ?? remembered ?? this.targets[0];
        if (initial) this.chooseInitial(initial.id);
        this.error =
          this.targets.length === 0
            ? "No pages are shared with your integration. In Notion, open a page's ••• menu → Connections and add it."
            : null;
      } catch (error) {
        this.error = describe(error);
      } finally {
        this.loading = false;
        task.notify();
      }
    });
  }

  targetById(id) {
    return this.targets.find((each) => each.id === id) ?? null;
  }

  errorNotice() {
    return this.error ? [new FormDescription("Notion", this.error)] : [];
  }

  /** Runs `work`, then reports with a toast that can open `page`; a pushed form returns to the list. */
  save(cx, progress, work, done) {
    if (this.saving) return;
    this.saving = true;
    cx.notify();
    show_toast({ title: progress, style: "progress", id: "save" });
    cx.spawn(async (task) => {
      try {
        const { page, target } = await work();
        this.remember(target.id);
        this.saving = false;
        this.afterSave();
        task.notify();
        if (this.target) pop();
        const choice = await show_toast({
          title: done,
          message: titleOf(page.object ? page : target),
          style: "success",
          id: "save",
          primary_action: "Open",
        });
        if (choice === "primary") open(page.url ?? target.url);
      } catch (error) {
        this.saving = false;
        task.notify();
        show_toast({ title: "Notion did not save it", message: describe(error), style: "failure", id: "save" });
      }
    });
  }

  /** Clears the fields after a save; a subclass resets its FormState. */
  afterSave() {}
}
