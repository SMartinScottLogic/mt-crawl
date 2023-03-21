find . -iname "*.~?~" | while read i
do
  if [[ -f "${i%%.~?~}" ]]
  then
    diff -urw --minimal <( cat "${i}" | jq -S '. | del(.links,.keywords,.story_id,.page)' ) <( cat "${i%%.~?~}" | jq -S '. | del(.links,.keywords,.story_id,.page)' ) && rm -fv "${i}"
  else
    mv -vn -- "${i}" "${i%%.~?~}"
  fi
done

