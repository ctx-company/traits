#!/usr/bin/env crystal
require "option_parser"

# The CLI's own exit contract, not this script's. A parked task must be
# REPORTED as parked rather than surfacing as a bare non-zero code, and the
# code is re-raised unchanged so batch callers keep the distinction.
EXIT_MEANINGS = {
  3 => "PARKED — run ended without completing (wall park, blocked, or refusal)",
  4 => "merge parked, branch and worktree intact",
  5 => "merge failed",
}

trait_id = "implement:quick"
tasks = [] of String

OptionParser.parse do |parser|
  parser.banner = %(usage: implement.cr [--trait <id>] "<task>[, <task>...]")
  parser.on("-t ID", "--trait=ID", "Trait or family:variant to run (default: #{trait_id})") { |id| trait_id = id }
  parser.on("-h", "--help", "Show this help") { puts parser; exit 0 }
  parser.unknown_args do |rest|
    tasks = rest.join(' ').split(',').map(&.strip).reject(&.empty?)
  end
end

abort %(no tasks given — see --help), 2 if tasks.empty?

completed = [] of String

tasks.each do |task|
  puts "=== implement: #{task} ==="

  # stdio is inherited so `--progress tui` keeps the real terminal: capturing
  # it here to inspect output would blank the live view.
  #
  # The TUI is only meaningful on a real terminal. Piped or redirected (CI, a
  # test harness, `just implement ... | tee`), it collapses to a terse summary
  # and the run's park/blocked detail never reaches the log — so the mode is
  # chosen from the actual stream, not assumed.
  progress = STDOUT.tty? ? "tui" : "stream"
  status = begin
    Process.run(
      "ctx",
      ["traits", "run", trait_id, "--set", "task=#{task}", "--worktree", "--merge", "--progress", progress, "--verbose"],
      input: Process::Redirect::Inherit,
      output: Process::Redirect::Inherit,
      error: Process::Redirect::Inherit,
    )
  rescue File::NotFoundError
    abort "cannot run `ctx` — is it on PATH?", 127
  end

  if status.success?
    completed << task
    next
  end

  code = status.exit_code
  reason = EXIT_MEANINGS.fetch(code, "ctx traits run exited #{code}")
  puts "=== STOPPED at #{task}: #{reason} (exit #{code}) ==="
  puts "=== completed before stop: #{completed.empty? ? "none" : completed.join(", ")} ==="
  exit code
end

puts "=== all tasks completed: #{completed.join(", ")} ==="
