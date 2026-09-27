import { useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { friendsApi, useFriends } from "@/lib/friends";
import type { AcceptClips, FriendsConfig } from "@/lib/types";
import { Button } from "@/components/ui/Button";
import { Card, SectionTitle } from "@/components/ui/Card";
import { ConfirmDelete } from "@/components/ui/ConfirmDelete";
import { Segmented, Toggle } from "@/components/ui/Controls";
import { Row } from "./shared";

export function FriendsTab() {
  const { friends, patchConfig } = useEngine(
    useShallow((s) => ({ friends: s.config.friends, patchConfig: s.patchConfig })),
  );
  const { signedIn, me, blocked } = useFriends(
    useShallow((s) => ({ signedIn: s.signedIn, me: s.me, blocked: s.lists.blocked })),
  );
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const patch = (change: Partial<FriendsConfig>) => patchConfig({ friends: { ...friends, ...change } });
  const run = (action: Promise<void>) => {
    setError(null);
    action.catch((err) => setError(String(err)));
  };

  return (
    <div className="space-y-8">
      <section>
        <SectionTitle title="Privacy" />
        <Card className="divide-y divide-line">
          <Row label="Appear offline" hint="You still see your friends; they see you as offline">
            <Toggle checked={friends.invisible} onChange={(invisible) => patch({ invisible })} />
          </Row>
          <Row label="Show what I'm playing" hint="Off: friends only see that you are online">
            <Toggle
              checked={friends.shareGame}
              disabled={friends.invisible}
              onChange={(shareGame) => patch({ shareGame })}
            />
          </Row>
          <Row
            label="Accept friend requests"
            hint={signedIn ? "Off: nobody can send you new requests" : "Sign in on the Friends page first"}
          >
            <Toggle
              checked={me?.allowRequests ?? true}
              disabled={!signedIn}
              onChange={(allow) => run(friendsApi.setAllowRequests(allow))}
            />
          </Row>
        </Card>
      </section>

      <section>
        <SectionTitle title="Clips from friends" />
        <Card className="divide-y divide-line">
          <Row
            label="Who can send me clips"
            hint="Straight from their PC to yours — you still say yes to each one"
          >
            <Segmented<AcceptClips>
              value={friends.acceptClips}
              options={[
                { key: "all", label: "Friends" },
                { key: "favorites", label: "Favorites" },
                { key: "off", label: "Nobody" },
              ]}
              onChange={(acceptClips) => patch({ acceptClips })}
            />
          </Row>
          <Row
            label="Tag friends on my clips"
            hint="Friends playing the same game when you clip go on it as a tag, like “with Luca”"
          >
            <Toggle checked={friends.tagFriends} onChange={(tagFriends) => patch({ tagFriends })} />
          </Row>
          <Row label="Busy" hint="No notices pop up, and clips sent meanwhile are turned down">
            <Toggle checked={friends.busy} onChange={(busy) => patch({ busy })} />
          </Row>
        </Card>
      </section>

      <section>
        <SectionTitle title="Notifications" />
        <Card className="divide-y divide-line">
          <Row label="Friend requests" hint="New requests, and when someone accepts yours">
            <Toggle checked={friends.notifyRequests} onChange={(notifyRequests) => patch({ notifyRequests })} />
          </Row>
          <Row label="A friend comes online">
            <Toggle checked={friends.notifyOnline} onChange={(notifyOnline) => patch({ notifyOnline })} />
          </Row>
          <Row label="A friend starts a game">
            <Toggle checked={friends.notifyGames} onChange={(notifyGames) => patch({ notifyGames })} />
          </Row>
          <Row
            label="Notify me while I'm playing"
            hint="On: a small ClippiBoy banner over the game. Off: nothing pops up — the console (Alt+C) still shows it all"
          >
            <Toggle
              checked={friends.notifyWhilePlaying}
              onChange={(notifyWhilePlaying) => patch({ notifyWhilePlaying })}
            />
          </Row>
        </Card>
      </section>

      {signedIn && (
        <section>
          <SectionTitle title="Blocked" />
          <Card className="divide-y divide-line">
            {blocked.length === 0 ? (
              <p className="p-5 text-sm text-ink-muted">Nobody. Block someone from the menu on their name.</p>
            ) : (
              blocked.map((user) => (
                <Row key={user.id} label={user.displayName} hint={`@${user.username}`}>
                  <Button size="sm" variant="ghost" onClick={() => run(friendsApi.unblock(user.id))}>
                    Unblock
                  </Button>
                </Row>
              ))
            )}
          </Card>
        </section>
      )}

      {signedIn && me && (
        <section>
          <SectionTitle title="Account" />
          <Card className="divide-y divide-line">
            <Row label={`Signed in as ${me.displayName}`} hint={`With Discord, @${me.username}`}>
              <Button size="sm" onClick={() => run(friendsApi.signOut())}>
                Sign out
              </Button>
            </Row>
            <Row label="Delete account" hint="Removes you, your friends list and your requests from the server">
              {confirmDelete ? (
                <ConfirmDelete
                  origin="right"
                  question="Delete account?"
                  confirmTitle="Delete it for good"
                  cancelLabel="Keep the account"
                  cancelTitle="Keep it — Escape does the same"
                  onConfirm={() => {
                    setConfirmDelete(false);
                    run(friendsApi.deleteAccount());
                  }}
                  onCancel={() => setConfirmDelete(false)}
                />
              ) : (
                <Button size="sm" variant="danger" onClick={() => setConfirmDelete(true)}>
                  Delete
                </Button>
              )}
            </Row>
          </Card>
        </section>
      )}

      {error && <Card className="border-live/40 bg-live/8 p-4 text-sm text-live">{error}</Card>}
    </div>
  );
}
