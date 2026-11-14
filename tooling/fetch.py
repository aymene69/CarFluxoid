"""Fetch pinned public sources to a gitignored cache; never fetch device identities."""
from pathlib import Path
import hashlib,json,urllib.request,shutil,tarfile,zipfile,subprocess,os,re
ROOT=Path(__file__).resolve().parents[1]
def sha(path):
 h=hashlib.sha256()
 with Path(path).open('rb') as f:
  for b in iter(lambda:f.read(1024*1024),b''):h.update(b)
 return h.hexdigest()
def fetch():
 lock=json.loads((ROOT/'tooling/sources.lock.json').read_text());cache=ROOT/'local/downloads';cache.mkdir(parents=True,exist_ok=True)
 for name,s in lock['sources'].items():
  dest=cache/s['file']
  if dest.exists() and sha(dest)==s['sha256']:continue
  tmp=dest.with_suffix('.partial');print('Downloading',name,flush=True)
  with urllib.request.urlopen(s['url'],timeout=180) as response,tmp.open('wb') as f:shutil.copyfileobj(response,f)
  if sha(tmp)!=s['sha256']:tmp.unlink();raise RuntimeError(f'{name}: checksum mismatch')
  tmp.replace(dest)
 return lock
if __name__=='__main__':fetch();print('Pinned source cache verified.')
