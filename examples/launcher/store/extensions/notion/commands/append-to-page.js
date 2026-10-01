// A form that adds text at the end of a Notion page, as paragraphs or to-dos.
// The form is `lib/append-form.js`, which Search Notion pushes too.
import { AppendForm } from "../lib/append-form.js";

export default class AppendToPage extends AppendForm {
  init(props, cx) {
    super.init({}, cx);
  }
}
