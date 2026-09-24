import { AppShell } from "./shell/AppShell.js";
import { InspectorZone } from "./shell/zones/InspectorZone.js";
import { LibraryZone } from "./shell/zones/LibraryZone.js";
import { PlayerZone } from "./shell/zones/PlayerZone.js";
import { TimelineZone } from "./shell/zones/TimelineZone.js";
import { ProjectLifecycle } from "./project/ProjectLifecycle.js";
import { SourcesBanner } from "./project/SourcesBanner.js";
import { UpdateBanner } from "./UpdateBanner.js";

/**
 * The application.
 *
 * Almost nothing: it composes the shell out of the four zones and gets out of
 * the way. The shell (#19) is a layout and knows nothing about what is in a
 * zone; each zone is filled by its own epic — the library by #53, the player by
 * Epic #4, the inspector by #49 and #56, the timeline by Epic #5.
 *
 * Every zone element is created here, once, and this component has no state, so
 * those elements keep their identity for the life of the application. That is
 * half of why a splitter drag re-renders nothing; the other half is in
 * `AppShell`, which never subscribes to the value a drag changes.
 */
export function App() {
  return (
    <>
      <UpdateBanner />
      <ProjectLifecycle />
      <SourcesBanner />
      <AppShell
        library={<LibraryZone />}
        player={<PlayerZone />}
        inspector={<InspectorZone />}
        timeline={<TimelineZone />}
      />
    </>
  );
}
