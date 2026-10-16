"use client";

import { Icon } from "../icons";
import { Preferences } from "../neptune/preferences";
import { usePrefs } from "../prefs";

function Setting({
  name,
  value,
  tone,
}: {
  name: string;
  value: string;
  tone: number;
}) {
  return (
    <div className="term-row">
      {name}
      <span className="text-muted"> = </span>
      {/* A changed value fades in, so the edit is easy to spot. */}
      <span key={value} className="animate-fade-in" style={{ color: `var(--ansi-${tone})` }}>
        {value}
      </span>
    </div>
  );
}

/** The saved settings, as `config.toml` holds them. */
function ConfigPane() {
  const { prefs } = usePrefs();
  const prompt = (
    <div className="term-row">
      <span style={{ color: "var(--ansi-4)", fontWeight: 700 }}>~/.config/neptune</span>
    </div>
  );
  const chevron = (
    <Icon name="chevronRight" size={14} className="inline-block size-[0.95em] align-[-0.1em] text-[var(--ansi-2)]" />
  );
  return (
    <div className="rounded-window bg-chrome p-1.5 shadow-window">
      <div
        data-focused="true"
        className="min-h-[264px] overflow-hidden rounded-pane bg-bg px-3 py-2.5 shadow-[inset_0_0_0_1px_var(--separator)]"
      >
        <div className="term" aria-label="config.toml" role="img">
          {prompt}
          <div className="term-row">{chevron} cat config.toml</div>
          <Setting name="theme" value={`"${prefs.theme.id}"`} tone={2} />
          <Setting name="window_zoom" value={prefs.windowZoom.toFixed(1)} tone={3} />
          <Setting name="font_size" value={prefs.fontSize.toFixed(1)} tone={3} />
          <Setting name="line_height" value={String(prefs.lineHeight)} tone={3} />
          <Setting name="scrollback" value={String(prefs.scrollback)} tone={3} />
          {/* Omitted, the shell is the platform's default. */}
          {prefs.shell && <Setting name="shell" value={`"${prefs.shell}"`} tone={2} />}
          <Setting name="cursor" value={`"${prefs.cursor}"`} tone={2} />
          <Setting name="cursor_blink" value={String(prefs.blink)} tone={5} />
          <Setting name="restore_workspaces" value={String(prefs.restoreWorkspaces)} tone={5} />
          <Setting name="confirm_close" value={String(prefs.confirmClose)} tone={5} />
          <Setting
            name="warn_running_processes"
            value={String(prefs.warnProcesses)}
            tone={5}
          />
          <Setting name="check_updates" value={String(prefs.checkUpdates)} tone={5} />
          <Setting name="release_channel" value={`"${prefs.releaseChannel}"`} tone={2} />
          <Setting
            name="desktop_notifications"
            value={String(prefs.desktopNotifications)}
            tone={5}
          />
          {prompt}
          <div className="term-row">
            {chevron} <span className="cursor" />
          </div>
        </div>
      </div>
    </div>
  );
}

export function Customize() {
  return (
    <section
      id="customize"
      className="mx-auto grid w-full max-w-[1180px] items-center gap-x-12 gap-y-10 px-5 sm:px-8 xl:grid-cols-[minmax(0,1fr)_minmax(0,680px)]"
    >
      <div className="min-w-0">
        <h2 className="text-[clamp(30px,4.4vw,44px)] leading-[1.05] font-semibold tracking-[-0.035em] text-balance">
          Make it yours.
        </h2>
        <p className="mt-4 max-w-[26rem] text-[16px] text-secondary">
          715 themes for the window and every terminal, settings you can search, and type set
          the way you read. Try them: this whole page follows.
        </p>
        <div className="mt-8 max-w-[30rem]">
          <ConfigPane />
        </div>
      </div>

      <div
        role="group"
        aria-label="Preferences"
        className="h-[520px] w-full max-w-[680px] justify-self-center overflow-hidden rounded-sheet border border-edge bg-elevated shadow-sheet select-none xl:justify-self-end"
      >
        <Preferences />
      </div>
    </section>
  );
}
