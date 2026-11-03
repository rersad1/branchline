import {readFile,writeFile} from 'node:fs/promises';
import {join,resolve} from 'node:path';
const version=process.argv[2];
if(!/^\d+\.\d+\.\d+$/.test(version??''))throw Error('usage: node scripts/set-version.mjs MAJOR.MINOR.PATCH');
const root=resolve(import.meta.dirname,'..');
for(const path of ['desktop/tauri.conf.json','frontend/package.json','frontend/package-lock.json','desktop/package.json','desktop/package-lock.json']){
  const file=join(root,path),data=JSON.parse(await readFile(file,'utf8'));
  data.version=version;if(data.packages?.[''])data.packages[''].version=version;
  await writeFile(file,JSON.stringify(data,null,2)+'\n');
}
for(const directory of ['backend','desktop']){
  const file=join(root,directory,'Cargo.toml'),content=await readFile(file,'utf8');
  await writeFile(file,content.replace(/^version = "[^"]+"/m,`version = "${version}"`));
  const lock=join(root,directory,'Cargo.lock'),lockContent=await readFile(lock,'utf8');
  await writeFile(lock,lockContent.replace(/(\[\[package\]\]\r?\nname = "agent-workspace-(?:backend|desktop)"\r?\nversion = ")[^"]+("\r?\n)/g,`$1${version}$2`));
}
console.log(`Application and lockfile versions set to ${version}`);
