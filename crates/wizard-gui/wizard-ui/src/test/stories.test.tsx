import { afterEach, beforeAll, describe, it } from "vitest";
import { cleanup } from "@testing-library/react";
import { composeStories, setProjectAnnotations } from "@storybook/react-vite";
import preview from "../../.storybook/preview";
import * as forms from "../components/catalyst/Forms.stories";
import * as menus from "../components/catalyst/Menus.stories";
import * as overlays from "../components/catalyst/Overlays.stories";
import * as flows from "../preview/Flows.stories";

const project = setProjectAnnotations(preview);
beforeAll(project.beforeAll);
afterEach(cleanup);

// Run the catalog's actual play functions so examples and regression coverage
// cannot silently diverge. Geometry and contrast are checked in the browser.
const groups = {
  forms: composeStories(forms),
  menus: composeStories(menus),
  overlays: composeStories(overlays),
  flows: composeStories(flows),
};
for (const [group, stories] of Object.entries(groups)) {
  describe(group, () => {
    for (const [name, story] of Object.entries(stories)) {
      if (story.play)
        it.skipIf(story.parameters.browserOnly)(name, async () => {
          await story.run();
        });
    }
  });
}
