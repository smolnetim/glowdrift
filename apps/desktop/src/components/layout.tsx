import { type ReactNode, useEffect, useRef } from "react";
import { Icon } from "./Icon";

export function Page({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => heading.current?.focus({ preventScroll: true }), []);
  return (
    <div className="page">
      <header className="page-header">
        <h1 ref={heading} tabIndex={-1}>
          {title}
        </h1>
        {description && <p className="page-description">{description}</p>}
      </header>
      {children}
    </div>
  );
}

export function Card({ title, description, children }: { title?: string; description?: string; children: ReactNode }) {
  return (
    <section className="card">
      {(title || description) && (
        <header className="card-header">
          {title && <h2>{title}</h2>}
          {description && <p className="setting-description">{description}</p>}
        </header>
      )}
      {children}
    </section>
  );
}

export function Banner({
  tone,
  title,
  children,
}: {
  tone: "danger" | "warning" | "info";
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className={`banner banner--${tone}`} role={tone === "danger" ? "alert" : "status"}>
      <Icon name={tone === "info" ? "info" : "alert"} />
      <div>
        <div className="banner-title">{title}</div>
        {children && <div className="banner-body">{children}</div>}
      </div>
    </div>
  );
}
