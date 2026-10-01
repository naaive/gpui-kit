// A form that creates a Notion page under a page or database shared with the
// integration: a title and text, one paragraph per line. Typing a title after
// the command's name in the root search fills it in. The form is
// `lib/page-form.js`, which Search Notion pushes too.
import { launch } from "launcher/api";
import { PageForm } from "../lib/page-form.js";

export default class QuickNote extends PageForm {
  init(props, cx) {
    super.init({ title: String(launch().arguments.title ?? "") }, cx);
  }
}
