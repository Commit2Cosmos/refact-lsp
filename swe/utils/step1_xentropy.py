import json
import os
import asyncio
import traceback
import numpy as np

from argparse import ArgumentParser

from swe.utils import AgentRunner
from swe.utils import get_swe_bench_lite_instance
from swe.steps import ExploreRepoStep, Locate
from swe.utils.common import patched_file
from swe.utils.common import filename_mentioned

from pathlib import Path
from typing import Dict, Any, Tuple


# MODEL = "gpt-4o"
MODEL = "gpt-4o-mini"


def softmax(x):
    exp_x = np.exp(x - np.max(x))
    return exp_x / exp_x.sum(axis=0)


def calculate_cross_entropy(true_filename: str, found_files_tochange: Dict[str, Dict[str, Any]]) -> float:
    fn_vec = sorted(found_files_tochange.keys())
    prob_gt = [0.0] * len(fn_vec)
    model_ratings = [found_files_tochange[fn]["RELEVANCY"] for fn in fn_vec]

    if not fn_vec:  # no files are generated by the model
        prob_gt = [0.0]
        model_ratings = [5]    # rating 5 to a wrong file
    for i, fn in enumerate(fn_vec):
        if true_filename in fn:
            prob_gt[i] = 1.0
            break
    else:
        prob_gt.append(1.0)
        model_ratings.append(1)

    prob_gt = np.array(prob_gt)
    model_ratings = np.array(model_ratings)

    model_ratings = np.clip(model_ratings, 1, 5)   # no zeros
    prob_rat = softmax(model_ratings)
    assert np.isclose(prob_gt.sum(), 1.0)
    assert np.isclose(prob_rat.sum(), 1.0)
    cross_entropy = - np.sum(prob_gt * np.log(prob_rat))
    if cross_entropy == -0.0:
        cross_entropy = 0.0
    print("prob_rat", prob_rat)
    print("prob_gt", prob_gt)
    print("cross_entropy", cross_entropy)
    # Example for non-existent file:
    #  prob_rat [0.98201379 0.01798621]
    #  prob_gt [0. 1.]
    #  cross_entropy 4.01814992791781
    return cross_entropy


class StepOneOnlyRunner(AgentRunner):

    async def _steps(self, base_url: str, repo_path: Path, *args, **kwargs) -> Tuple[Dict[str, Any], str]:
        results: Dict[str, Any] = dict()
        problem_statement = kwargs["problem_statement"]
        true_filename: str = patched_file(kwargs["problem_patch"])
        results["patched_file"] = true_filename
        results["patched_file_mentioned_in_problem"] = filename_mentioned(true_filename, problem_statement)
        rf = Locate(base_url=base_url, model_name=MODEL, attempts=1)
        found_files: Dict[str, Dict[str, Any]]    # {"filename": {"prop1": value, "prop2": value}, ...}
        try:
            found_files, symbols = await rf.process(
                problem_statement=problem_statement,
                repo_path=repo_path)
        except Exception as e:
            raise e
            results["error"] = f"step1: {type(e)} {str(e) or traceback.format_exc()}"
            found_files = {}
        if isinstance(found_files, list):
            found_files_tochange = {d["file_path"]: d for d in found_files if d["reason"] == "to_change"}
            found_files_list = [d["file_path"] for d in found_files_tochange.values()]
            for d in found_files_tochange.values():
                d["RELEVANCY"] = 5
        else:
            found_files_tochange = {k: d for k, d in found_files.items() if d["WHY_CODE"] == "TOCHANGE"}
            found_files_list = list(found_files_tochange.keys())
        results["found_files"] = found_files_list
        results["patched_file_is_found"] = filename_mentioned(true_filename, "\n".join(found_files_list))
        results["model_name"] = rf.model_name
        results["usage"] = rf.usage
        # def calculate_cross_entropy(true_filename: str, found_files_tochange: Dict[str, Dict[str, Any]]) -> float:
        results["cross_entropy"] = calculate_cross_entropy(true_filename, found_files_tochange)
        return results, rf.trajectory


async def main():
    parser = ArgumentParser()
    # django__django-11039
    parser.add_argument("instance_id", type=str, help="SWE instance id")
    parser.add_argument("--timeout", type=float, default=None, help="processing timeout")
    parser.add_argument("--output-dir", type=Path, required=True, help="output directory")
    args = parser.parse_args()
    args.output_dir.mkdir(exist_ok=True, parents=True)

    out_fn_json = args.output_dir / f"{args.instance_id}.json"
    if out_fn_json.exists():
        print(f"skip, because {out_fn_json} already exists")
        return

    instance = get_swe_bench_lite_instance(args.instance_id)
    run_postfix = f"-{args.output_dir.name}" if args.output_dir is not None else ""
    results = {
        "model_name_or_path": f"refact-dev-{MODEL}{run_postfix}",
        "instance_id": args.instance_id,
        "problem_statement": instance["problem_statement"],
        "problem_patch": instance["patch"],
    }
    traj = ""

    try:
        runner = StepOneOnlyRunner(
            timeout=args.timeout,
            use_ast=True,
            use_vecdb=False,
        )
        r, traj = await runner.run(
            repo_name=instance["repo"],
            base_commit=instance["base_commit"],
            output_dir=args.output_dir,
            **results,
        )
        results.update(**r, **results)
    except Exception as e:
        raise e
        results["error"] = str(e) or traceback.format_exc()

    cross_entropy = results.get("cross_entropy", 9.0)
    with open(args.output_dir / ("%s-%0.3f.md" % (args.instance_id, cross_entropy)), "w") as f:
        f.write(traj)
    with open(out_fn_json, "w") as f:
        json.dump(results, f, indent=4)
    return results


if __name__ == "__main__":
    asyncio.run(main())
