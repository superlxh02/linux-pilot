"""导入两种实验模式共享的版本化拓扑，保留用户之后的显式修改。"""

import copy
import json
import os
import sys
import urllib.error
import urllib.request


BASE = "http://127.0.0.1:3000/api/v1/topology/manifest"
HEADERS = {
    "Authorization": "Bearer " + os.environ["PILOT_OPERATOR_TOKEN"],
    "Content-Type": "application/json",
}


def request(method, payload=None):
    body = json.dumps(payload, ensure_ascii=False).encode() if payload is not None else None
    command = urllib.request.Request(BASE, body, HEADERS, method=method)
    with urllib.request.urlopen(command, timeout=12) as response:
        return json.load(response)


def main():
    template = json.load(open(sys.argv[1], encoding="utf-8"))
    try:
        current = request("GET")
        if current is None:
            result = request("PUT", template)
            print("已导入双模式拓扑：", result)
        elif current["metadata"]["id"] == "linux-pilot-sim" and current["metadata"]["revision"] == 1:
            # 唯一允许自动升级的旧版样例。用户调过版本或换过 id 后，
            # 脚本不会覆盖其配置；可在控制台可视化编辑或导入 JSON。
            upgraded = copy.deepcopy(template)
            upgraded["metadata"]["revision"] = 2
            result = request("PUT", upgraded)
            print("旧版单模式样例已升级为双模式拓扑：", result)
        else:
            print("保留当前拓扑配置；如需合并实验节点，请在控制台编辑或导入")
    except urllib.error.URLError as error:
        raise SystemExit(f"拓扑导入失败：{error}")


if __name__ == "__main__":
    main()
