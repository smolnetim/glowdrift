import { useEffect, useState } from "react";
import { Card, Page } from "../components/layout";
import { Logo } from "../components/Icon";
import { type AppInfo, getAppInfo } from "../lib/ipc";

export function About() {
  const [info, setInfo] = useState<AppInfo | null>(null);

  useEffect(() => {
    getAppInfo()
      .then(setInfo)
      .catch(() => setInfo(null));
  }, []);

  return (
    <Page title="About">
      <Card>
        <div className="about-head">
          <Logo size={48} />
          <div>
            <div className="about-name">{info?.name ?? "Glowdrift"}</div>
            <div className="muted-text">{info ? `Version ${info.version}` : "Version unavailable"}</div>
          </div>
        </div>
        <dl className="facts">
          <dt>Licence</dt>
          <dd>MIT</dd>
          <dt>Source</dt>
          <dd>
            <code>github.com/smolnetim/glowdrift</code>
          </dd>
        </dl>
      </Card>
      <Card title="Privacy">
        <p>
          Screen content is processed in memory on this PC and is never stored or sent anywhere. Only the final
          colour goes to your lights.
        </p>
      </Card>
    </Page>
  );
}
