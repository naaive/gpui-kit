// A menu-bar command: the unread notification count beside a tray icon, and
// the latest few in its menu. The launcher runs it again every five minutes,
// and the root search shows the count as its subtitle.
import { View } from "gpui-kit";
import { Action, MenuBarExtra, MenuBarItem, MenuBarSection, MenuBarSeparator } from "launcher";
import { update_command_metadata } from "launcher/api";
import { Query } from "launcher/utils";
import { messageOf, token } from "../lib/github.js";
import { INBOX_URL, loadNotifications } from "../lib/notifications.js";

const SHOWN = 8;

export default class UnreadNotifications extends View {
  init(props, cx) {
    this.token = token();
    if (!this.token) return;
    const secret = this.token;
    this.notifications = new Query(
      cx,
      async () => {
        const notifications = await loadNotifications(secret);
        update_command_metadata({ subtitle: notifications.length === 0 ? null : `${notifications.length} unread` });
        return notifications;
      },
      { cache_key: "notifications", failure_toast: false },
    );
  }

  latest() {
    const notifications = this.notifications.data ?? [];
    if (notifications.length === 0) {
      const text = this.notifications.error
        ? `Cannot load: ${messageOf(this.notifications.error)}`
        : this.notifications.loading
          ? "Loading…"
          : "All caught up";
      return [new MenuBarItem(text)];
    }
    const items = notifications.slice(0, SHOWN).map((notification) =>
      new MenuBarItem(notification.title)
        .subtitle(notification.repository)
        .action(new Action("Open in Browser").open_url(notification.url)),
    );
    if (notifications.length > SHOWN) items.push(new MenuBarItem(`${notifications.length - SHOWN} more`));
    return items;
  }

  render() {
    const footer = [
      new MenuBarSeparator(),
      new MenuBarItem("Open Notifications").action(new Action("Open Notifications").launch("notifications")),
      new MenuBarItem("Open on GitHub").action(new Action("Open on GitHub").open_url(INBOX_URL)),
    ];
    if (!this.token) {
      return new MenuBarExtra()
        .icon("bell")
        .tooltip("GitHub: add a token in preferences")
        .children([new MenuBarItem("Add a token in the GitHub extension's preferences"), ...footer]);
    }
    const count = (this.notifications.data ?? []).length;
    return new MenuBarExtra()
      .icon(count > 0 ? "bell-dot" : "bell")
      .title(count > 0 ? String(count) : "")
      .tooltip(count === 1 ? "GitHub: 1 unread notification" : `GitHub: ${count} unread notifications`)
      .loading(this.notifications.loading)
      .children([
        new MenuBarSection("Unread").children(this.latest()),
        ...footer,
        new MenuBarItem("Reload").action(new Action("Reload").run((cx) => this.notifications.revalidate(cx))),
      ]);
  }
}
