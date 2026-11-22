import { useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { friendsApi, useFriends } from "@/lib/friends";
import type { AcceptClips, FriendsConfig } from "@/lib/types";
import { Button } from "@/components/ui/Button";
import { Card, SectionTitle } from "@/components/ui/Card";
import { ConfirmDelete } from "@/components/ui/ConfirmDelete";
import { Segmented, Toggle } from "@/components/ui/Controls";
import { blockedSummary } from "@/lib/settingsSummary";
import { Row, SettingsGroup } from "./shared";

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
        <SectionTitle title="What friends see" />
        <Card className="divide-y divide-line">
          <Row label="Show what I'm playing">
            <Toggle checked={friends.shareGame} onChange={(shareGame) => patch({ shareGame })} />
          </Row>
          <Row label="Show ClippiBoy in my Discord status">
            <Toggle
              checked={friends.discordStatus ?? true}
              onChange={(discordStatus) => patch({ discordStatus })}
            />
          </Row>
        </Card>
      </section>

      <section>
        <SectionTitle title="Requests" />
        <Card className="divide-y divide-line">
          <Row label="Accept friend requests">
            <Toggle
              checked={me?.allowRequests ?? true}
              disabled={!signedIn}
              onChange={(allow) => run(friendsApi.setAllowRequests(allow))}
            />
          </Row>
          <Row label="My name on share links">
            <Toggle checked={friends.nameOnLinks} onChange={(nameOnLinks) => patch({ nameOnLinks })} />
          </Row>
          <Row label="Requests from my share links">
            <Toggle
              checked={friends.friendOnLinks ?? true}
              disabled={!friends.nameOnLinks}
              onChange={(friendOnLinks) => patch({ friendOnLinks })}
            />
          </Row>
        </Card>
      </section>

      <section>
        <SectionTitle title="Clips from friends" />
        <Card className="divide-y divide-line">
          <Row label="Who can send me clips">
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
          <Row label="Tag friends on my clips">
            <Toggle checked={friends.tagFriends} onChange={(tagFriends) => patch({ tagFriends })} />
          </Row>
        </Card>
      </section>

      {signedIn && (
        <section>
          <Card className="divide-y divide-line">
            <SettingsGroup title="Blocked" storageKey="friends.blocked" summary={blockedSummary(blocked.length)}>
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
            </SettingsGroup>
            {me && (
              <SettingsGroup title="Account" storageKey="friends.account" summary={`Signed in as ${me.displayName}`}>
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
              </SettingsGroup>
            )}
          </Card>
        </section>
      )}

      {error && <Card className="border-live/40 bg-live/8 p-4 text-sm text-live">{error}</Card>}
    </div>
  );
}
