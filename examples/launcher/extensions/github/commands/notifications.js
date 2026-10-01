// Your unread GitHub notifications, newest first. Open one on github.com,
// mark it read, or mark them all read. Without a token the page says how to
// add one.
import { View } from "gpui-kit";
import { Action, ActionPanel, ActionPanelSection, List, ListItem } from "launcher";
import { show_toast, update_command_metadata } from "launcher/api";
import { Query } from "launcher/utils";
import { NO_TOKEN, messageOf, noTokenItem, token } from "../lib/github.js";
import { INBOX_URL, loadNotifications, markRead } from "../lib/notifications.js";

function countText(count) {
  return count === 0 ? null : `${count} unread`;
}

export default class Notifications extends View {
  init(props, cx) {
    this.token = token();
    if (!this.token) return;
    const secret = this.token;
    this.notifications = new Query(
      cx,
      async () => {
        const notifications = await loadNotifications(secret);
        update_command_metadata({ subtitle: countText(notifications.length) });
        return notifications;
      },
      { cache_key: "notifications", failure_title: "Cannot load notifications" },
    );
  }

  remove(ids) {
    const left = (this.notifications.data ?? []).filter((item) => !ids.has(item.id));
    this.notifications.mutate(left);
    update_command_metadata({ subtitle: countText(left.length) });
  }

  mark(threads, all, cx) {
    const count = threads.length;
    cx.spawn(async (task) => {
      try {
        await markRead(this.token, threads, all);
        this.remove(new Set(threads.map((thread) => thread.id)));
        show_toast({ title: count === 1 ? "Marked as read" : `Marked ${count} as read`, style: "success" });
      } catch (error) {
        show_toast({ title: "Cannot mark as read", message: messageOf(error), style: "failure" });
      }
      task.notify();
    });
  }

  row(notification) {
    return new ListItem(notification.id, notification.title)
      .icon(notification.icon)
      .subtitle(notification.repository)
      .keyword(notification.type)
      .tag(notification.reason)
      .accessory_tooltip("Why you were notified")
      .accessory_date(notification.updated)
      .actions(
        new ActionPanel().children([
          new Action("Open in Browser").open_url(notification.url),
          new Action("Mark as Read").icon("check").shortcut("secondary-shift-m").run((cx) => this.mark([notification], false, cx)),
          new Action("Copy URL").icon("copy").shortcut("secondary-shift-c").copy(notification.url),
          new ActionPanelSection("All Notifications").children([
            new Action("Mark All as Read…")
              .icon("check-check")
              .shortcut("secondary-shift-a")
              .confirm("Mark All as Read?", "Every unread notification is marked read on GitHub.")
              .run((cx) => this.mark(this.notifications.data ?? [], true, cx)),
            new Action("Open Notifications on GitHub").icon("inbox").open_url(INBOX_URL),
            new Action("Reload").icon("refresh-cw").shortcut("secondary-r").run((cx) => this.notifications.revalidate(cx)),
          ]),
        ]),
      );
  }

  emptyState() {
    if (this.notifications.loading && !this.notifications.data) return ["Loading notifications…", "Asking GitHub"];
    if (this.notifications.error) return ["Cannot load notifications", messageOf(this.notifications.error)];
    return ["All caught up", "You have no unread notifications."];
  }

  render() {
    if (!this.token) {
      return new List().empty_title(NO_TOKEN.title).empty_description(NO_TOKEN.description).child(noTokenItem());
    }
    const [title, description] = this.emptyState();
    return new List()
      .placeholder("Filter notifications…")
      .loading(this.notifications.loading)
      .empty_title(title)
      .empty_description(description)
      .children((this.notifications.data ?? []).map((notification) => this.row(notification)));
  }
}
