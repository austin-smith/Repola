import type { CommitPerson, CommitTrailer } from "../types";

export function parseCommitPeople(value: string, label: string): CommitPerson[] {
  return value
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const match = /^(.+?)\s*<([^<>]+)>$/.exec(line);
      if (!match) throw new Error(label + " must use the format Name <email@example.com>, one per line.");
      return { name: match[1].trim(), email: match[2].trim() };
    });
}

export function parseCommitTrailers(value: string): CommitTrailer[] {
  return value
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const separator = line.indexOf(":");
      if (separator <= 0 || separator === line.length - 1) {
        throw new Error("Trailers must use the format Key: value, one per line.");
      }
      return { key: line.slice(0, separator).trim(), value: line.slice(separator + 1).trim() };
    });
}
