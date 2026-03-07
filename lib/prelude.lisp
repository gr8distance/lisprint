;; lisprint prelude — 暗黙ロードされる標準ライブラリ

;; --- 制御 ---

(defmacro when (cond body)
  `(if ~cond ~body nil))

(defmacro unless (cond body)
  `(if (not ~cond) ~body nil))

;; --- コレクション ---

(defun map (f lst)
  (if (empty? lst)
    '()
    (cons (f (first lst))
          (map f (rest lst)))))

(defun filter (pred lst)
  (if (empty? lst)
    '()
    (if (pred (first lst))
      (cons (first lst) (filter pred (rest lst)))
      (filter pred (rest lst)))))

(defun reduce (f init lst)
  (if (empty? lst)
    init
    (reduce f (f init (first lst)) (rest lst))))

(defun each (f lst)
  (if (not (empty? lst))
    (do
      (f (first lst))
      (each f (rest lst))))
  nil)

(defun reject (pred lst)
  (filter (fn (x) (not (pred x))) lst))

(defun find (pred lst)
  (if (empty? lst)
    nil
    (if (pred (first lst))
      (first lst)
      (find pred (rest lst)))))

(defun flatten (lst)
  (if (empty? lst)
    '()
    (if (list? (first lst))
      (concat (flatten (first lst)) (flatten (rest lst)))
      (cons (first lst) (flatten (rest lst))))))

;; --- ユーティリティ ---

(defun inc (n) (+ n 1))
(defun dec (n) (- n 1))
(defun zero? (n) (= n 0))
(defun pos? (n) (> n 0))
(defun neg? (n) (< n 0))
(defun even? (n) (= (mod n 2) 0))
(defun odd? (n) (not (even? n)))

(defun comp (f g)
  (fn (x) (f (g x))))

(defun range (n)
  (loop [i 0 acc '()]
    (if (= i n)
      acc
      (recur (+ i 1) (concat acc (list i))))))
