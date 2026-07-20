import { clientPackageName } from "@mag/client";
import { Button } from "@mag/ui";
import "@mag/ui/styles.css";

/** Minimal web shell rendered by the Vite app skeleton. */
export function App(): React.JSX.Element {
  return (
    <main className="min-h-screen bg-background p-8 text-foreground">
      <section className="mx-auto flex max-w-3xl flex-col gap-4 rounded-lg border border-border bg-white/80 p-6 shadow-sm">
        <p className="text-sm font-medium uppercase tracking-wide text-primary">mag web</p>
        <h1 className="text-3xl font-semibold">Shared UI workspace ready</h1>
        <p className="text-sm text-foreground/70">
          The web shell depends on @mag/ui and {clientPackageName}; transport wiring lands in W3-2.
        </p>
        <div>
          <Button type="button">New chat</Button>
        </div>
      </section>
    </main>
  );
}
