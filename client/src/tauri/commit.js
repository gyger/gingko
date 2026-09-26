// Port of src/electron/commit.js using a browser-compatible sha1.
import { sha1hex } from './sha1.js'

async function writeTree (workingTree) {
  const treeObjects = []

  async function addShaIds (tree) {
    if (tree.children.length === 0) {
      const shaId = sha1hex(tree.content + '\n')
      treeObjects.push({ _id: shaId, type: 'tree', content: tree.content, children: [] })
      return Object.assign(tree, { _id: shaId })
    } else {
      const childrenWithShas = await Promise.all(tree.children.map(async t => await addShaIds(t)))
      const str = tree.content + '\n' + childrenWithShas.map(c => c._id + ' ' + c.id).join('\n')
      const shaId = sha1hex(str)
      treeObjects.push({ _id: shaId, type: 'tree', content: tree.content, children: childrenWithShas.map(c => [c._id, c.id]) })
      return Object.assign(tree, { _id: shaId })
    }
  }

  await addShaIds(workingTree)
  return treeObjects
}

export default async function commitTree (author, parents, tree, timestamp, metadata) {
  const treeObjects = await writeTree(tree)
  const rootId = treeObjects[treeObjects.length - 1]._id
  const str = rootId + '\n' +
    (parents.length === 1 ? parents[0] + '\n' : parents.join('\n')) +
    author + ' ' + (timestamp.toString())
  const commitSha = sha1hex(str)
  const commitObj = { _id: commitSha, type: 'commit', tree: rootId, parents: parents, author: author, timestamp: timestamp }
  return [commitSha, treeObjects.concat([commitObj]), metadata]
}
