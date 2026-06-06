import urllib.request
import json
import sys
url = 'https://api.github.com/repos/namefailed/phoneme/actions/runs'
req = urllib.request.Request(url)
try:
    with urllib.request.urlopen(req) as response:
        data = json.loads(response.read().decode())
        run = data['workflow_runs'][0]
        print(f"{run['status']} {run['conclusion']}")
except Exception as e:
    print(e)
