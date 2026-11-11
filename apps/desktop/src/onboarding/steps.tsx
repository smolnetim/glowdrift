import type { ReactNode } from "react";
import { ModeChoice, SmoothingControl } from "../components/ColorFeel";
import { Icon, type IconName } from "../components/Icon";
import { LiveSwatch } from "../components/LiveColor";
import { OutputsPanel } from "../components/Outputs";
import { ScreenRegionPicker } from "../components/ScreenRegionPicker";
import { AutoUpdateToggle, StartMinimisedToggle, StartWithWindowsToggle } from "../components/StartupToggles";

export interface Step {
  id: string;
  title: string;
  summary: string;
  body: ReactNode;
}

function HowItWorks() {
  const items: { icon: IconName; title: string; text: string }[] = [
    { icon: "capture", title: "Capture", text: "Watches a part of your screen." },
    { icon: "wave", title: "Calm", text: "Turns it into one smooth colour." },
    { icon: "lights", title: "Glow", text: "Sends it to your lights." },
  ];
  return (
    <ol className="how-it-works">
      {items.map((item, index) => (
        <li key={item.title}>
          <span className="how-icon">
            <Icon name={item.icon} size={24} />
          </span>
          <div className="how-title">{item.title}</div>
          <div className="muted-text">{item.text}</div>
          {index < items.length - 1 && (
            <span className="how-arrow" aria-hidden="true">
              <Icon name="arrowRight" size={18} />
            </span>
          )}
        </li>
      ))}
    </ol>
  );
}

export const STEPS: readonly Step[] = [
  {
    id: "welcome",
    title: "Welcome to Glowdrift",
    summary:
      "Glowdrift captures a part of your screen and calms it into one smooth colour. It then sends that colour to your lights.",
    body: (
      <div className="stack">
        <HowItWorks />
        <p className="muted-text">
          Setup takes about a minute. Everything runs on this PC, and your screen is never saved or sent anywhere.
        </p>
      </div>
    ),
  },
  {
    id: "screen",
    title: "Pick your screen and region",
    summary: "Choose a display, then drag the frame over the part you want your lights to follow.",
    body: <ScreenRegionPicker />,
  },
  {
    id: "lights",
    title: "Connect your lights",
    summary: "Glowdrift looks for supported lights on this PC.",
    body: <OutputsPanel />,
  },
  {
    id: "feel",
    title: "Set the feel",
    summary: "Slow the colour down so changes fade in gently. The swatch below follows your screen live.",
    body: (
      <div className="stack">
        <LiveSwatch label="Live preview of the smoothed colour" />
        <SmoothingControl />
        <div className="stack-tight">
          <div className="section-label">How to pick the colour</div>
          <ModeChoice />
        </div>
      </div>
    ),
  },
  {
    id: "finish",
    title: "Almost done",
    summary: "Choose how Glowdrift starts. You can change any of this later in Settings.",
    body: (
      <div className="card card--flush">
        <StartWithWindowsToggle />
        <StartMinimisedToggle />
        <AutoUpdateToggle />
      </div>
    ),
  },
];
