import { useEffect, useRef, useState } from "react";
import { Icon, Logo } from "../components/Icon";
import { patchConfig } from "../state/config";
import { STEPS } from "./steps";

const finish = () => patchConfig({ onboardingComplete: true });

export function Onboarding() {
  const [index, setIndex] = useState(0);
  const heading = useRef<HTMLHeadingElement>(null);
  const step = STEPS[index];
  const isFirst = index === 0;
  const isLast = index === STEPS.length - 1;

  useEffect(() => heading.current?.focus({ preventScroll: true }), [index]);

  if (!step) return null;

  return (
    <div className="onboarding">
      <div className="app-glow" aria-hidden="true" />
      <header className="onboarding-top">
        <div className="onboarding-inner onboarding-bar">
          <div className="brand">
            <Logo size={26} />
            <span>Glowdrift setup</span>
          </div>
          <div className="onboarding-progress">
            <span className="muted-text">
              Step {index + 1} of {STEPS.length}
            </span>
            <progress
              className="progress"
              value={index + 1}
              max={STEPS.length}
              aria-label={`Setup progress, step ${index + 1} of ${STEPS.length}`}
            />
          </div>
        </div>
      </header>

      <main className="onboarding-scroll">
        <div className="onboarding-inner onboarding-main">
          <h1 ref={heading} tabIndex={-1}>
            {step.title}
          </h1>
          <p className="page-description">{step.summary}</p>
          <div className="onboarding-body">{step.body}</div>
        </div>
      </main>

      <footer className="onboarding-footer">
        <div className="onboarding-inner onboarding-bar">
          <button type="button" className="btn btn--subtle" onClick={finish}>
            Skip setup
          </button>
          <div className="onboarding-nav">
            {!isFirst && (
              <button type="button" className="btn" onClick={() => setIndex(index - 1)}>
                <Icon name="arrowLeft" size={16} />
                Back
              </button>
            )}
            <button
              type="button"
              className="btn btn--primary"
              onClick={isLast ? finish : () => setIndex(index + 1)}
            >
              {isLast ? "Done" : "Next"}
              {!isLast && <Icon name="arrowRight" size={16} />}
            </button>
          </div>
        </div>
      </footer>
    </div>
  );
}
