import type { SessionStore, SessionStoreSnapshot } from "@mag/client";
import { SourcesView } from "@mag/ui";
import * as React from "react";

/** Props for the sources page container. */
export interface SourcesPageProps {
  /** Store used for source commands. */
  readonly store: SessionStore;
  /** Source list from the store snapshot. */
  readonly sources: SessionStoreSnapshot["sources"];
  /** Called when the user navigates back to the session route. */
  readonly onBack: () => void;
}

/**
 * Container for the sources table (docs/WEB.md §5.5): refreshes the source
 * list on entry and probes local agents on demand. The probed list replaces
 * the store's sources, so the table updates through the snapshot.
 */
export function SourcesPage({ onBack, sources, store }: SourcesPageProps): React.JSX.Element {
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState<string>();

  React.useEffect(() => {
    store.refreshSources().catch((refreshError: unknown) => {
      setError(errorMessage(refreshError));
    });
  }, [store]);

  const probe = async (): Promise<void> => {
    setBusy(true);
    setError(undefined);
    try {
      await store.probeSources();
    } catch (probeError: unknown) {
      setError(errorMessage(probeError));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-2">
      {error !== undefined ? (
        <p className="mx-4 mt-4 rounded-lg border border-destructive/20 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </p>
      ) : null}
      <SourcesView busy={busy} sources={sources} onBack={onBack} onProbe={() => void probe()} />
    </div>
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
