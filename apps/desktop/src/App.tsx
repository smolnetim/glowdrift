import { useEffect, useState } from "react";
import { Icon, type IconName, Logo } from "./components/Icon";
import { Banner } from "./components/layout";
import { UpdatePill } from "./components/Updates";
import { frontendReady } from "./lib/ipc";
import { useStore } from "./lib/store";
import { configStore, loadConfig } from "./state/config";
import { Onboarding } from "./onboarding/Onboarding";
import { About } from "./views/About";
import { Capture } from "./views/Capture";
import { Dashboard } from "./views/Dashboard";
import { Lights } from "./views/Lights";
import { Settings } from "./views/Settings";

type ViewId = "dashboard" | "capture" | "lights" | "settings" | "about";

const NAV: readonly { id: ViewId; label: string; icon: IconName }[] = [
  { id: "dashboard", label: "Dashboard", icon: "dashboard" },
  { id: "capture", label: "Capture", icon: "capture" },
  { id: "lights", label: "Lights", icon: "lights" },
  { id: "settings", label: "Settings", icon: "settings" },
  { id: "about", label: "About", icon: "info" },
];

function ViewContent({ view }: { view: ViewId }) {
  switch (view) {
    case "dashboard":
      return <Dashboard />;
    case "capture":
      return <Capture />;
    case "lights":
      return <Lights />;
    case "settings":
      return <Settings />;
    case "about":
      return <About />;
  }
}

function Shell() {
  const [view, setView] = useState<ViewId>("dashboard");
  const { saveFailed } = useStore(configStore);

  return (
    <div className="shell">
      <div className="app-glow" aria-hidden="true" />
      <aside className="sidebar">
        <div className="brand">
          <Logo size={28} />
          <span>Glowdrift</span>
        </div>
        <nav aria-label="Main">
          <ul className="nav-list">
            {NAV.map((item) => (
              <li key={item.id}>
                <button
                  type="button"
                  className="nav-item"
                  aria-current={view === item.id ? "page" : undefined}
                  onClick={() => setView(item.id)}
                >
                  <Icon name={item.icon} />
                  {item.label}
                </button>
              </li>
            ))}
          </ul>
        </nav>
        <div className="sidebar-footer">
          <UpdatePill />
        </div>
      </aside>
      <main className="content">
        {saveFailed && (
          <Banner tone="warning" title="Your last change could not be saved">
            Glowdrift will try again the next time you change a setting.
          </Banner>
        )}
        <ViewContent view={view} />
      </main>
    </div>
  );
}

function Splash({ failed }: { failed: boolean }) {
  return (
    <div className="splash">
      <div className="app-glow" aria-hidden="true" />
      <Logo size={48} />
      {failed ? (
        <>
          <p>Glowdrift could not load its settings.</p>
          <button type="button" className="btn btn--primary" onClick={() => void loadConfig()}>
            Try again
          </button>
        </>
      ) : (
        <p className="muted-text">Loading&hellip;</p>
      )}
    </div>
  );
}

/** Tells the shell once the first real screen has been painted, so the window never shows blank. */
function useSignalReady(ready: boolean): void {
  useEffect(() => {
    if (!ready) return;
    let second = 0;
    const first = requestAnimationFrame(() => {
      second = requestAnimationFrame(() => void frontendReady().catch(() => undefined));
    });
    return () => {
      cancelAnimationFrame(first);
      cancelAnimationFrame(second);
    };
  }, [ready]);
}

export default function App() {
  const { config, loadFailed } = useStore(configStore);
  useSignalReady(config !== null || loadFailed);
  if (!config) return <Splash failed={loadFailed} />;
  return config.onboardingComplete ? <Shell /> : <Onboarding />;
}
