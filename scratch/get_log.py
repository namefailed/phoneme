import urllib.request
import json
import zipfile
import io

url = 'https://api.github.com/repos/namefailed/phoneme/actions/runs/27071621062/logs'
req = urllib.request.Request(url)
try:
    with urllib.request.urlopen(req) as response:
        with zipfile.ZipFile(io.BytesIO(response.read())) as thezip:
            for zipinfo in thezip.infolist():
                if 'Rust' in zipinfo.filename:
                    print(f"--- {zipinfo.filename} ---")
                    lines = thezip.read(zipinfo).decode('utf-8').splitlines()
                    for line in lines:
                        if 'error' in line.lower() or 'warn' in line.lower() or 'fail' in line.lower():
                            print(line)
except Exception as e:
    print(e)
