#!/usr/bin/env python3
"""Redistribution notices must include native vendors and embedded font authors."""
import hashlib,importlib.util,json,pathlib,tempfile,unittest
path=pathlib.Path(__file__).resolve().parents[1]/'release'/'package.py'
spec=importlib.util.spec_from_file_location('genoryn_package',path);package=importlib.util.module_from_spec(spec);spec.loader.exec_module(package)
class DependencyLicenses(unittest.TestCase):
 def test_inventory_follows_resolved_target_not_inactive_lockfile_packages(self):
  native={'id':'native','name':'native','source':'registry'};inactive={'id':'other-platform','name':'other-platform','source':'registry'};workspace={'id':'workspace','name':'project','source':None}
  metadata={'packages':[native,inactive,workspace],'resolve':{'nodes':[{'id':'native'},{'id':'workspace'}]}}
  self.assertEqual(package.packages_for_target(metadata),[native])
  with self.assertRaisesRegex(ValueError,'Missing'):package.packages_for_target({'packages':[native]})
  with self.assertRaisesRegex(ValueError,'Incomplete'):package.packages_for_target({'packages':[native],'resolve':{'nodes':[{'id':'unknown'}]}})
 def test_vendored_third_party_dependency_is_included_but_workspace_is_not(self):
  owned={'id':'project','source':None};vendor={'id':'vendor','source':None};registry={'id':'registry','source':'registry'}
  metadata={'packages':[owned,vendor,registry],'workspace_members':['project'],'resolve':{'nodes':[{'id':p['id']}for p in [owned,vendor,registry]]}}
  self.assertEqual(package.packages_for_target(metadata),[vendor,registry])
 def test_vendored_native_source_and_wrapper_match_recorded_provenance(self):
  provenance=package.validate_vendored_sources(path.parents[2])
  self.assertEqual(set(provenance['patched_files']),{'src/lib.rs','build.rs'})
  self.assertTrue(provenance['native_randomx_source_unchanged'])
 def test_every_crates_io_patch_has_verified_provenance(self):
  provenance=package.validate_all_vendored_sources(path.parents[2])
  self.assertEqual([record['name']for record in provenance],['randomx-rs','saturating-time'])
  self.assertEqual(set(provenance[1]['patched_files']),{'src/internal.rs','src/lib.rs'})
  patch=path.parents[2]/provenance[1]['patch']
  self.assertEqual(hashlib.sha256(patch.read_bytes()).hexdigest(),provenance[1]['patch_sha256'])
 def test_unrecorded_crates_io_patch_is_rejected(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory)
   (root/'Cargo.toml').write_text('[patch.crates-io]\nrandomx-rs = { path = "third-party/randomx-rs" }\nsaturating-time = { path = "third-party/saturating-time" }\nunreviewed = { path = "elsewhere" }\n')
   with self.assertRaisesRegex(ValueError,'Unrecorded'):package.validate_all_vendored_sources(root)
 def test_vendored_unrecorded_build_input_is_rejected(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory);vendor=root/'third-party/randomx-rs';vendor.mkdir(parents=True)
   (vendor/'source.c').write_bytes(b'pinned source')
   record={'original_file_sha256':{'source.c':hashlib.sha256(b'pinned source').hexdigest()},'patched_files':{}}
   (root/'third-party/randomx-source.json').write_text(json.dumps(record))
   package.validate_vendored_sources(root)
   (vendor/'extra.c').write_text('unrecorded build input')
   with self.assertRaisesRegex(ValueError,'inventory'):package.validate_vendored_sources(root)
 def test_all_packaged_project_document_links_resolve(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory);package.copy_documentation(root,path.parents[2]);package.check_document_links(root)
   self.assertTrue((root/'DEVELOPMENT_STATUS.md').is_file());self.assertTrue((root/'crates/genoryn-connect/README.md').is_file())
 def test_broken_packaged_document_link_is_rejected(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory);(root/'README.md').write_text('[missing guide](docs/absent.md)')
   with self.assertRaisesRegex(ValueError,'Broken packaged'):package.check_document_links(root)
 def test_nested_notices_preserve_paths_and_authorship(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory)/'crate';root.mkdir();destination=pathlib.Path(directory)/'package'
   notices={'LICENSE':'binding authors','RandomX/LICENSE':'native authors','fonts/OFL.txt':'Noto authors','fonts/UFL.txt':'Ubuntu authors','fonts/emoji-icon-font-mit-license.txt':'emoji authors','fonts/Hack-Regular.txt':'Hack authors','vendor/NOTICE':'notice','vendor/licenses/MIT':'vendored MIT notice','legal/terms.md':'declared license'}
   for name,text in notices.items():
    file=root/name;file.parent.mkdir(parents=True,exist_ok=True);file.write_text(text)
   (root/'fonts/Hack-Regular.ttf').write_bytes(b'test font placeholder')
   (root/'README.md').write_text('not a license');(root/'notes.txt').write_text('not a font notice')
   package.copy_dependency_licenses(root,destination,'legal/terms.md')
   actual={file.relative_to(destination).as_posix():file.read_text()for file in destination.rglob('*')if file.is_file()}
   self.assertEqual(actual,notices)
 def test_pointer_files_and_license_checker_scripts_are_not_notice_coverage(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory)
   for name,text in {'LICENSE.MIT':'../LICENSE.MIT','LICENSE.APACHE':'../LICENSE.APACHE\n','.license.ignore':'','file_license_check.sh':'#!/bin/sh\ncat LICENSE\n','LICENSE-ID':'MIT'}.items():(root/name).write_text(text)
   self.assertEqual(package.dependency_license_files(root),[])
   (root/'LICENSE').write_text('Copyright (c) Example Authors\nPermission is hereby granted, free of charge.\n')
   self.assertEqual([relative for relative,_ in package.dependency_license_files(root)],[pathlib.Path('LICENSE')])
 def test_declared_path_is_normalized_and_deduplicated(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory);(root/'legal').mkdir();(root/'LICENSE').write_text('authors')
   selected=package.dependency_license_files(root,'legal/../LICENSE')
   # Windows may supply an 8.3 TEMP alias; the collector returns canonical paths.
   self.assertEqual(selected,[(pathlib.Path('LICENSE'),(root/'LICENSE').resolve())])
 def test_outside_and_oversized_files_are_excluded(self):
  with tempfile.TemporaryDirectory()as directory:
   base=pathlib.Path(directory);root=base/'crate';root.mkdir();(base/'LICENSE').write_text('outside')
   (root/'LICENSE-huge').write_bytes(b'x'*1_000_000)
   self.assertEqual(package.dependency_license_files(root,'../LICENSE'),[])
 def test_symlink_escape_is_excluded(self):
  with tempfile.TemporaryDirectory()as directory:
   base=pathlib.Path(directory);root=base/'crate';root.mkdir();outside=base/'outside';outside.write_text('outside')
   try:(root/'LICENSE').symlink_to(outside)
   except (OSError,NotImplementedError):self.skipTest('Host does not allow unprivileged symlinks')
   self.assertEqual(package.dependency_license_files(root),[])
 def test_supplement_is_version_bound_and_checksummed(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory);source=root/'upstream';source.mkdir();(source/'LICENSE').write_text('upstream authors')
   entry={'filename':'LICENSE','sha256':hashlib.sha256(b'upstream authors').hexdigest(),'packages':[{'name':'dep','version':'1.0'}]}
   count=package.copy_supplemental_licenses({'name':'dep','version':'1.0'},root/'out',source,[entry]);self.assertEqual(count,1)
   self.assertEqual((root/'out/upstream-supplement/LICENSE').read_text(),'upstream authors')
   self.assertEqual(package.copy_supplemental_licenses({'name':'dep','version':'2.0'},root/'wrong-version',source,[entry]),0)
   (source/'LICENSE').write_text('modified')
   with self.assertRaisesRegex(ValueError,'checksum'):package.copy_supplemental_licenses({'name':'dep','version':'1.0'},root/'bad',source,[entry])
 def test_supplement_rejects_parent_path(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory);entry={'filename':'../LICENSE','sha256':'ignored','packages':[{'name':'dep','version':'1'}]}
   with self.assertRaisesRegex(ValueError,'Unsafe'):package.copy_supplemental_licenses({'name':'dep','version':'1'},root/'out',root,[entry])
 def test_standard_license_cannot_replace_a_different_declaration(self):
  with tempfile.TemporaryDirectory()as directory:
   root=pathlib.Path(directory);entry={'source_kind':'canonical-license-text','spdx_license':'CC0-1.0','filename':'CC0.txt','sha256':'irrelevant','packages':[{'name':'dep','version':'1'}]}
   with self.assertRaisesRegex(ValueError,'does not match'):package.copy_supplemental_licenses({'name':'dep','version':'1','license':'MIT'},root/'out',root,[entry])
 def test_committed_supplement_bytes_match_provenance_on_every_platform(self):
  root=path.parents[2]/'third-party'/'license-supplements'
  records=json.loads((root/'manifest.json').read_text(encoding='utf-8'))
  self.assertTrue(records)
  with tempfile.TemporaryDirectory()as directory:
   destination=pathlib.Path(directory)
   for record in records:
    self.assertEqual(package.copy_supplemental_licenses(record['packages'][0],destination,root,[record]),1)
if __name__=='__main__':unittest.main()
