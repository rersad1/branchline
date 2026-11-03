// Prepare only public release artifacts. Private signing keys never enter this directory.
import {readFile,writeFile,mkdir,copyFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join,resolve} from 'node:path';
const root=resolve(import.meta.dirname,'..');
const config=JSON.parse(await readFile(join(root,'desktop/tauri.conf.json'),'utf8'));
const version=config.version;
const tag=process.env.GITHUB_REF_TYPE==='tag'?process.env.GITHUB_REF_NAME:`v${version}`;
if(!/^\d+\.\d+\.\d+$/.test(version)||tag!==`v${version}`)throw Error('Release tag must match the stable application version');
for(const directory of ['frontend','desktop']){
  const p=JSON.parse(await readFile(join(root,directory,'package.json'),'utf8'));
  if(p.version!==version)throw Error(`${directory} package version differs from the release`);
}
for(const directory of ['backend','desktop']){
  const cargo=await readFile(join(root,directory,'Cargo.toml'),'utf8');
  if(cargo.match(/^version = "([^"]+)"/m)?.[1]!==version)throw Error(`${directory} Cargo version differs from the release`);
}
const target=process.env.CARGO_TARGET_DIR?resolve(root,process.env.CARGO_TARGET_DIR):join(root,'desktop/target');
const installer=join(target,'release/bundle/nsis',`${config.productName}_${version}_x64-setup.exe`);
const signature=(await readFile(`${installer}.sig`,'utf8')).trim();
const signatureText=Buffer.from(signature,'base64').toString('utf8');
const comment=signatureText.split(/\r?\n/).find(line=>line.startsWith('trusted comment: '))?.slice('trusted comment: '.length);
if(comment?.split('\t').find(value=>value.startsWith('version:'))!==`version:${version}`)throw Error('Updater signature must bind the release version');
const output=join(root,'release-artifacts');await mkdir(output,{recursive:true});
const name=`Branchline_${version}_x64-setup.exe`;
await copyFile(installer,join(output,name));await writeFile(join(output,`${name}.sig`),`${signature}\n`);
const platform={url:`https://github.com/rersad1/branchline/releases/download/v${version}/${name}`,signature};
const notes=await readFile(join(root,'docs/releases',`${version}.md`),'utf8');
await writeFile(join(output,'latest.json'),JSON.stringify({version,notes,pub_date:new Date().toISOString(),platforms:{'windows-x86_64':platform,'windows-x86_64-nsis':platform}},null,2)+'\n');
const names=[name,`${name}.sig`,'latest.json'];
const hashes=await Promise.all(names.map(async file=>`${createHash('sha256').update(await readFile(join(output,file))).digest('hex')}  ${file}`));
await writeFile(join(output,'SHA256SUMS.txt'),hashes.join('\n')+'\n');
console.log(JSON.stringify({version,directory:output,files:[...names,'SHA256SUMS.txt']}));
