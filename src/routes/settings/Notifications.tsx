import { BannerTab } from "./Banner";
import { SoundsTab } from "./Sounds";

/** Banner and Sounds one under the other until they become one list. */
export function NotificationsTab() {
  return (
    <div className="space-y-8">
      <BannerTab />
      <SoundsTab />
    </div>
  );
}
