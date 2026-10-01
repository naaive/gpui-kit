// How a page or database looks in a list, a detail pane and a dropdown.
// A list item shows the page's emoji or image as its icon; a dropdown, which
// has no icons, puts the emoji before the title.
import { DropdownItem } from "launcher";
import { iconOf, titleOf } from "./notion.js";

export function fallbackIcon(object) {
  return object.object === "database" ? "database" : "file-text";
}

/** The title with the page's emoji before it, if it has one. */
export function label(object) {
  const icon = iconOf(object);
  const title = titleOf(object);
  return icon?.emoji ? `${icon.emoji} ${title}` : title;
}

/** The page's emoji or `https://` image as the item's icon, or the Lucide fallback. */
export function itemIcon(object) {
  const icon = iconOf(object);
  if (icon?.emoji) return icon.emoji;
  return icon?.image && icon.image.startsWith("https://") ? icon.image : fallbackIcon(object);
}

/** Dropdown items for parents a new page can go under. */
export function parentItems(objects) {
  return objects.map(
    (object) => new DropdownItem(object.id, object.object === "database" ? `${label(object)} (Database)` : label(object)),
  );
}
