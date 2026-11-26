import { describe, expect, it } from "vite-plus/test";
import { FEEDBACK_ISSUES_URL, buildFeedbackIssue, type FeedbackReport } from "./feedback";

const report: FeedbackReport = {
  appVersion: "2.6.0",
  platform: "Windows",
  language: "Italian",
  lyricsScript: "romanized",
  wordSync: false,
  song: {
    title: "1% Battery Girl (feat. ChiliChill)",
    artist: "Sān-Z & HOYO-MiX",
    album: "1% Battery Girl",
    durationMs: 210_400,
    player: "Spotify",
  },
  lyricsMode: "synced",
  lyrics: {
    trackName: "1% Low Battery Girl",
    artistName: "Sān-Z, HOYO-MiX",
    albumName: "",
    wordTimed: true,
    source: "Unison",
    lines: 59,
  },
};

describe("buildFeedbackIssue", () => {
  it("names the song in the title and fills the same sections every time", () => {
    const issue = buildFeedbackIssue(report);

    expect(issue.title).toBe("Lyrics: 1% Battery Girl (feat. ChiliChill) - Sān-Z & HOYO-MiX");
    for (const section of ["### What is wrong?", "### Song", "### What the app found", "### App"]) {
      expect(issue.body).toContain(section);
    }
    expect(issue.body).toContain("- Length: 3:30");
    expect(issue.body).toContain("- Matched: 1% Low Battery Girl - Sān-Z, HOYO-MiX");
    expect(issue.body).toContain("- Source: Unison");
    expect(issue.body).toContain("- Timing: word by word");
    expect(issue.body).toContain("- Version: 2.6.0 (Windows)");
  });

  it("opens the repository's new issue form with the title, body and label", () => {
    const issue = buildFeedbackIssue(report);
    const url = new URL(issue.url);

    expect(`${url.origin}${url.pathname}`).toBe(FEEDBACK_ISSUES_URL);
    expect(url.searchParams.get("title")).toBe(issue.title);
    expect(url.searchParams.get("body")).toBe(issue.body);
    expect(url.searchParams.get("labels")).toBe("lyrics-feedback");
  });

  it("says so when nothing was playing or nothing was found", () => {
    const issue = buildFeedbackIssue({
      ...report,
      song: null,
      lyrics: null,
      lyricsMode: "missing",
    });

    expect(issue.title).toBe("Feedback from the app");
    expect(issue.body).toContain("- Nothing was playing");
    expect(issue.body).toContain("- Result: No lyrics found");
    expect(issue.body).not.toContain("- Matched:");
  });

  it("keeps a line of the list on one line and the address short", () => {
    const issue = buildFeedbackIssue({
      ...report,
      song: { ...report.song!, title: `Long\nname ${"x".repeat(9_000)}` },
    });

    expect(issue.body).not.toMatch(/Title: Long\nname/);
    expect(issue.url.length).toBeLessThan(7_000);
    expect(new URL(issue.url).searchParams.get("body")).toContain("### Song");
  });
});
