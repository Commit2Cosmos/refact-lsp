import json
import subprocess

from swe.steps import Step
from refact import chat_client

from pathlib import Path
from typing import Dict, Any, List, Set


DONE_MESSAGE = "DONE"
SYSTEM_MESSAGE = f"""
You are Refact Dev, an auto coding assistant.

You'll receive a problem statement from user.
Your aim is to solve this problem using speculation over the code and analyzing outputs of given tools.
Use tools to get access to the codebase. Use each tool exact in it's format do not add any extra args.

A good strategy to solve the issue is:
1. Build context:
 - use file, definition or references tools
 - before you move to the next step, make sure you collect all needed context: file names, code, etc.
2. Speculate about the problem and solve it:
 - describe what changes you need to do
 - apply changes to files separately using patch tool
 - paths argument should be always full paths to given files within repo
 - do not generate the patch itself, use patch(paths, todo) tool to make this changes
3. When you are done with the task, send the message including only one word: {DONE_MESSAGE}

Changing tests is not allowed!

Explain your plan briefly before calling the tools in parallel.
IT IS FORBIDDEN TO JUST CALL TOOLS WITHOUT EXPLAINING. EXPLAIN FIRST! USE TOOLS IN PARALLEL!
"""


class ProducePatchStep(Step):
    def __init__(self, attempts: int, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self._attempts = attempts

    @property
    def _tools(self) -> Set[str]:
        return {
            "file",
            "definition",
            "patch",
        }

    async def _patch_generate(self, repo_name: Path, formatted_diff: List[Dict[str, Any]]):
        await chat_client.diff_apply(self._base_url, chunks=formatted_diff, apply=[True] * len(formatted_diff))
        result = subprocess.check_output(["git", "--no-pager", "diff"], cwd=str(repo_name))
        await chat_client.diff_apply(self._base_url, chunks=formatted_diff, apply=[False] * len(formatted_diff))
        # TODO: make sure that un-apply works well (possible freezes in refact-lsp and possible patches in repo after)
        # subprocess.check_output(["git", "stash"], cwd=str(repo_name))
        return result.decode()

    async def _single_step(self, message: str, repo_path: Path) -> str:
        messages = [
            chat_client.Message(role="system", content=SYSTEM_MESSAGE),
            chat_client.Message(role="user", content=message),
        ]

        for step_n in range(self._max_depth):
            print(f"{'-' * 40} step {step_n} {'-' * 40}")
            messages = await self._query(messages)
            applied_diff_call_ids = set()
            for m in [m for m in messages if m.role == "diff" and m.tool_call_id not in applied_diff_call_ids]:
                applied_diff_call_ids.add(m.tool_call_id)
                try:
                    formatted_diff = json.loads(m.content)
                    return await self._patch_generate(repo_path.absolute(), formatted_diff)
                except json.decoder.JSONDecodeError:
                    continue
            if messages[-1].role == "assistant" \
                    and messages[-1].content \
                    and DONE_MESSAGE == messages[-1].content:
                break
        raise RuntimeError(f"can't solve the problem with {self._max_depth} steps")

    async def process(self, task: str, repo_path: Path, **kwargs) -> List[str]:
        results = []
        for attempt_n in range(self._attempts):
            print(f"{'=' * 40} attempt {attempt_n} {'=' * 40}")
            try:
                results.append(await self._single_step(task, repo_path))
            except:
                continue
        if not results:
            raise RuntimeError(f"can't produce result with {self._attempts} attempts")
        return results