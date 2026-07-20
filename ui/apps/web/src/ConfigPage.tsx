import type { SessionStore } from "@mag/client";
import { ConfigEditor } from "@mag/ui";
import * as React from "react";

/** Props for the config page container. */
export interface ConfigPageProps {
  /** Store used for config commands. */
  readonly store: SessionStore;
  /** Called when the user navigates back to the session route. */
  readonly onBack: () => void;
}

/**
 * Container for the text-form ConfigEditor (docs/WEB.md §5.5): loads the
 * runtime config as TOML on entry, saves parsed edits, and wires Reload /
 * Apply. Errors stay page-local so TOML parse failures surface next to the
 * editor.
 */
export function ConfigPage({ onBack, store }: ConfigPageProps): React.JSX.Element {
  const [text, setText] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState<string>();

  React.useEffect(() => {
    let cancelled = false;
    setBusy(true);
    store
      .getConfigText()
      .then((configText) => {
        if (!cancelled) {
          setText(configText);
        }
      })
      .catch((loadError: unknown) => {
        if (!cancelled) {
          setError(errorMessage(loadError));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setBusy(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [store]);

  const run = async (action: () => Promise<void>): Promise<void> => {
    setBusy(true);
    setError(undefined);
    try {
      await action();
    } catch (actionError: unknown) {
      setError(errorMessage(actionError));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ConfigEditor
      busy={busy}
      error={error}
      text={text}
      onApply={() => void run(() => store.applyConfig())}
      onBack={onBack}
      onChange={setText}
      onReload={() =>
        void run(async () => {
          await store.reloadConfig();
          setText(await store.getConfigText());
        })
      }
      onSave={() => void run(() => store.saveConfigText(text))}
    />
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
